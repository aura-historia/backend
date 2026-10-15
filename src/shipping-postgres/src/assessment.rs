//! Bounded, current dependency reads. No evidence payloads or listing-geography projections.
use application::error::{BoxError, box_error};
use geo::{GeographicDescription, data::geography_data::GeographicDescriptionData};
use geo_service::resolution::{ResolutionReader, ResolutionSubject, RetentionPurpose, ScopeKind};
use listing_source_core::{ListingIngestionMethod, ListingSourceId};
use party_core::{party_id::PartyId, party_location::*, party_location_id::PartyLocationId};
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    delivery::{DeliveryIntegration, ListingDelivery},
    product_listing_id::ProductListingId,
};
use shipping_core::{
    assessment::EffectiveDeliveryTerms,
    profile::*,
    selection::{ProfileSelection, SourceDefaultProfile},
};
use shipping_service::assessment::*;
use sqlx::{FromRow, PgConnection};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxAssessmentReaderFactory;
struct Reader<'a>(&'a mut PgConnection);
impl AssessmentReaderFactory<SqlxTransaction> for SqlxAssessmentReaderFactory {
    fn in_transaction<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl AssessmentReader + 'a {
        Reader(tx.connection())
    }
}
fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> AssessmentError {
    AssessmentError::InvalidState(box_error(source))
}
fn invalid_box(source: BoxError) -> AssessmentError {
    AssessmentError::InvalidState(source)
}
fn corrupt() -> AssessmentError {
    invalid(std::io::Error::other(
        "invalid shipping assessment dependency",
    ))
}
fn storage(source: sqlx::Error) -> AssessmentError {
    AssessmentError::Storage(box_error(source))
}
fn resolution_error(error: geo_service::resolution::ResolutionError) -> AssessmentError {
    use geo_service::resolution::ResolutionError;
    match error {
        ResolutionError::Storage(source) => AssessmentError::Storage(source),
        ResolutionError::Transaction(source) => AssessmentError::Transaction(source),
        ResolutionError::InvalidState(source) => AssessmentError::InvalidState(source),
        other => invalid(other),
    }
}
fn revision(value: i64) -> Result<u64, AssessmentError> {
    u64::try_from(value)
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(corrupt)
}
fn evidence(
    verified_at: OffsetDateTime,
    fresh_until: Option<OffsetDateTime>,
    expires_at: Option<OffsetDateTime>,
    at: OffsetDateTime,
) -> Result<AssertionEvidence, AssessmentError> {
    if verified_at > at
        || fresh_until.is_some_and(|t| t < verified_at)
        || expires_at.is_some_and(|t| t < verified_at || fresh_until.is_some_and(|f| t < f))
    {
        return Err(corrupt());
    }
    Ok(AssertionEvidence {
        verified_at,
        fresh_until,
        expires_at,
    })
}
#[derive(FromRow)]
struct ListingRow {
    product_listing_id: Uuid,
    listing_source_id: Uuid,
    version: i64,
    lifecycle: String,
    delivery_configuration: Option<serde_json::Value>,
    source_revision: i64,
    operator_party_id: Uuid,
    delivery_verified_at: Option<OffsetDateTime>,
    delivery_fresh_until: Option<OffsetDateTime>,
    delivery_expires_at: Option<OffsetDateTime>,
    enabled_methods: Vec<String>,
    shopify_domain: Option<String>,
    woocommerce_domain: Option<String>,
}
#[derive(FromRow)]
struct ProfileRow {
    #[sqlx(flatten)]
    profile: super::ProfileRow,
    integration_method: Option<String>,
    integration_domain: Option<String>,
    verified_at: OffsetDateTime,
    fresh_until: Option<OffsetDateTime>,
    expires_at: Option<OffsetDateTime>,
}
#[derive(FromRow)]
struct LocationRow {
    party_location_id: Uuid,
    party_id: Uuid,
    geography: Option<serde_json::Value>,
    lifecycle: String,
    revision: i64,
    input_revision: i64,
}
#[async_trait::async_trait]
impl AssessmentReader for Reader<'_> {
    async fn snapshot(
        &mut self,
        ids: &[ProductListingId],
        user: Option<UserId>,
    ) -> Result<AssessmentDependencies, AssessmentError> {
        if ids.is_empty() || ids.len() > MAX_ASSESSMENT_LISTINGS {
            return Err(AssessmentError::InvalidInput);
        }
        // First operation in this service-owned read transaction, before the snapshot is taken.
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *self.0)
            .await
            .map_err(storage)?;
        let at: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *self.0)
            .await
            .map_err(storage)?;
        if let Some(user) = user {
            let active: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM users WHERE user_id=$1 AND NOT suspended)",
            )
            .bind(user.into_uuid())
            .fetch_one(&mut *self.0)
            .await
            .map_err(storage)?;
            if !active {
                return Err(AssessmentError::Forbidden);
            }
        }
        let rows = sqlx::query_as::<_, ListingRow>(r#"
            SELECT l.product_listing_id,l.listing_source_id,l.version,l.lifecycle,l.delivery_configuration,s.version AS source_revision,s.operator_party_id,
                l.delivery_verified_at,l.delivery_fresh_until,l.delivery_expires_at,
                ARRAY(SELECT ingestion_method FROM listing_source_ingestion_methods m WHERE m.listing_source_id=l.listing_source_id) AS enabled_methods,
                sh.domain AS shopify_domain,w.shipping_integration_domain AS woocommerce_domain
            FROM product_listings l JOIN listing_sources s USING(listing_source_id)
            LEFT JOIN listing_source_shopify_ingestion_configurations sh USING(listing_source_id)
            LEFT JOIN listing_source_woocommerce_ingestion_configurations w USING(listing_source_id)
            WHERE l.product_listing_id=ANY($1)
        "#).bind(ids.iter().map(|i| i.into_uuid()).collect::<Vec<_>>()).fetch_all(&mut *self.0).await.map_err(storage)?;
        let mut listings = Vec::new();
        for row in rows {
            match row.lifecycle.as_str() {
                "ACTIVE" => {}
                "WITHDRAWN" => continue,
                _ => return Err(corrupt()),
            }
            let source_id = ListingSourceId::try_from(row.listing_source_id).map_err(invalid)?;
            let has_configuration = row.delivery_configuration.is_some();
            let delivery = row
                .delivery_configuration
                .map(shipping_json::listing_delivery::decode::<super::terms::StoredReferences>)
                .transpose()
                .map_err(invalid_box)?
                .unwrap_or_else(|| ListingDelivery::inherited(source_id));
            if delivery.source_id() != source_id {
                return Err(corrupt());
            }
            let integration_current = match delivery.integration() {
                DeliveryIntegration::Source => true,
                DeliveryIntegration::Provider { method, domain } => {
                    row.enabled_methods.iter().any(|m| m == method.as_str())
                        && match method {
                            ListingIngestionMethod::Shopify => {
                                row.shopify_domain.as_deref() == Some(domain.as_str())
                            }
                            ListingIngestionMethod::Woocommerce => {
                                row.woocommerce_domain.as_deref() == Some(domain.as_str())
                            }
                            _ => false,
                        }
                }
            };
            let assertion = if has_configuration {
                Some(evidence(
                    row.delivery_verified_at.ok_or_else(corrupt)?,
                    row.delivery_fresh_until,
                    row.delivery_expires_at,
                    at,
                )?)
            } else {
                if row.delivery_verified_at.is_some()
                    || row.delivery_fresh_until.is_some()
                    || row.delivery_expires_at.is_some()
                {
                    return Err(corrupt());
                }
                None
            };
            listings.push(ListingDependency {
                id: ProductListingId::try_from(row.product_listing_id).map_err(invalid)?,
                source_id,
                revision: revision(row.version)?,
                source_revision: revision(row.source_revision)?,
                operator_id: PartyId::try_from(row.operator_party_id).map_err(invalid)?,
                delivery,
                evidence: assertion,
                integration_current,
            });
        }
        let sources: Vec<_> = listings.iter().map(|l| l.source_id.into_uuid()).collect();
        let default_rows: Vec<(Uuid,Option<Uuid>,i64,bool)> = sqlx::query_as("SELECT listing_source_id,shipping_profile_id,revision,source_deleted FROM shipping_source_defaults WHERE listing_source_id=ANY($1)").bind(&sources).fetch_all(&mut *self.0).await.map_err(storage)?;
        let mut defaults = Vec::new();
        for (source, id, rev, deleted) in default_rows {
            if deleted || rev < 2 {
                return Err(corrupt());
            }
            defaults.push(DefaultDependency {
                selection: SourceDefaultProfile::rehydrate(
                    ListingSourceId::try_from(source).map_err(invalid)?,
                    id.map(ShippingProfileId::try_from)
                        .transpose()
                        .map_err(invalid)?,
                ),
                revision: revision(rev)?,
            });
        }
        let mut profile_ids = Vec::new();
        for listing in &listings {
            let id = match listing.delivery.selection() {
                Some(ProfileSelection::Explicit(id)) => Some(*id),
                Some(ProfileSelection::InheritSourceDefault) => defaults
                    .iter()
                    .find(|d| d.selection.source_id() == listing.source_id)
                    .and_then(|d| d.selection.profile_id()),
                _ => None,
            };
            if let Some(id) = id
                && !profile_ids.contains(&id.into_uuid())
            {
                profile_ids.push(id.into_uuid());
            }
        }
        let rows = sqlx::query_as::<_, ProfileRow>(r#"
            SELECT shipping_profile_id,listing_source_id,management_revision,terms_revision,lifecycle,name,attributed_party_id,terms,
                integration_method,integration_domain,verified_at,fresh_until,expires_at
            FROM shipping_profiles WHERE shipping_profile_id=ANY($1) AND current_source_id=listing_source_id AND listing_source_id=ANY($2)
        "#).bind(&profile_ids).bind(&sources).fetch_all(&mut *self.0).await.map_err(storage)?;
        let mut profiles = Vec::new();
        for row in rows {
            let view = row.profile.view().map_err(invalid)?;
            let integration = match (row.integration_method, row.integration_domain) {
                (None, None) => DeliveryIntegration::Source,
                (Some(method), Some(domain)) => {
                    shipping_json::listing_delivery::decode_integration(
                        serde_json::json!({"kind":"PROVIDER","method":method,"domain":domain}),
                    )
                    .map_err(invalid_box)?
                }
                _ => return Err(corrupt()),
            };
            profiles.push(ProfileDependency {
                profile: ShippingProfile::rehydrate(
                    view.id,
                    view.source_id,
                    view.revision,
                    view.lifecycle,
                    view.content,
                )
                .map_err(invalid)?,
                management_revision: view.management_revision.into_inner(),
                integration,
                evidence: evidence(row.verified_at, row.fresh_until, row.expires_at, at)?,
            });
        }
        let mut location_ids = Vec::new();
        for listing in &listings {
            let default = defaults
                .iter()
                .find(|d| d.selection.source_id() == listing.source_id);
            let matching: Vec<_> = profiles
                .iter()
                .filter(|p| p.profile.source_id() == listing.source_id)
                .map(|p| p.profile.clone())
                .collect();
            let effective = EffectiveDeliveryTerms::resolve(
                &listing.delivery.configuration(),
                default.map(|d| &d.selection),
                &matching,
            );
            for id in referenced_locations(effective.terms()) {
                if !location_ids.contains(&id.into_uuid()) {
                    location_ids.push(id.into_uuid());
                }
            }
        }
        let rows = sqlx::query_as::<_, LocationRow>("SELECT party_location_id,party_id,geography,lifecycle,revision,input_revision FROM party_locations WHERE party_location_id=ANY($1)").bind(&location_ids).fetch_all(&mut *self.0).await.map_err(storage)?;
        let subjects = rows
            .iter()
            .map(|r| {
                ResolutionSubject::new(RetentionPurpose::PartyLocation, r.party_location_id)
                    .map_err(invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let resolutions = geo_postgres::SqlxResolutionRepositoryFactory::snapshot_reader(self.0)
            .inspect_many(&subjects)
            .await
            .map_err(resolution_error)?;
        if resolutions.len() != rows.len() {
            return Err(corrupt());
        }
        let mut locations = Vec::new();
        for (row, resolution) in rows.into_iter().zip(resolutions) {
            let geography = row
                .geography
                .map(serde_json::from_value::<GeographicDescriptionData>)
                .transpose()
                .map_err(invalid)?
                .map(Option::<GeographicDescription>::try_from)
                .transpose()
                .map_err(invalid)?
                .flatten();
            let rev = revision(row.revision)?;
            let input_revision =
                PartyLocationInputRevision::try_from(row.input_revision).map_err(invalid)?;
            if input_revision.into_inner() > rev {
                return Err(corrupt());
            }
            if let Some(r) = &resolution
                && (r.scope.kind() != ScopeKind::Party || r.scope.owner() != Some(row.party_id))
            {
                return Err(corrupt());
            }
            let resolved = resolution.as_ref().filter(|r| {
                r.input_revision == input_revision.into_inner()
                    && r.guard == rev
                    && row.lifecycle == "ACTIVE"
            });
            let derived = resolved
                .and_then(|r| r.accepted_geography())
                .and_then(|g| g.description());
            // Domestic comparison needs only a country. Submitted facts have precedence;
            // no street, postcode, point or provider payload leaves this reader.
            let submitted_country = geography
                .as_ref()
                .and_then(|g| g.country().or_else(|| g.subdivision().map(|s| s.country())));
            let derived_country =
                derived.and_then(|g| g.country().or_else(|| g.subdivision().map(|s| s.country())));
            let (geography, resolution_generation) = if let Some(country) = submitted_country {
                (
                    GeographicDescription::new_in_release(
                        geography
                            .as_ref()
                            .expect("submitted country")
                            .reference_release(),
                        None,
                        Some(country),
                        None,
                        None,
                    )
                    .map_err(invalid)?,
                    None,
                )
            } else if let Some(country) = derived_country {
                (
                    GeographicDescription::new_in_release(
                        derived.expect("derived country").reference_release(),
                        None,
                        Some(country),
                        None,
                        None,
                    )
                    .map_err(invalid)?,
                    resolved.map(|r| r.work.generation),
                )
            } else {
                (None, None)
            };
            locations.push(LocationDependency {
                location: PartyLocation::rehydrate(
                    PartyLocationId::try_from(row.party_location_id).map_err(invalid)?,
                    PartyId::try_from(row.party_id).map_err(invalid)?,
                    PartyLocationContent {
                        label: PartyLocationLabel::new("Delivery site".into()).map_err(invalid)?,
                        roles: Default::default(),
                        geography,
                        position: None,
                    },
                    PartyLocationLifecycle::from_code(&row.lifecycle).ok_or_else(corrupt)?,
                    input_revision,
                ),
                revision: rev,
                resolution_generation,
            });
        }
        Ok(AssessmentDependencies {
            evaluated_at: at,
            listings,
            profiles,
            defaults,
            locations,
        })
    }
}
