//! Bounded, current dependency reads. No evidence payloads or listing-geography projections.
use application::error::{BoxError, box_error};
use geo::{GeographicDescription, data::geography_data::GeographicDescriptionData};
use geo_service::resolution::{
    ResolutionReader, ResolutionSubject, ResolutionView, RetentionPurpose, ScopeKind,
};
use listing_source_core::ListingSourceId;
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
struct Reader<'a>(&'a mut PgConnection, bool);
impl AssessmentReaderFactory<SqlxTransaction> for SqlxAssessmentReaderFactory {
    fn in_transaction<'a>(&'a self, tx: &'a mut SqlxTransaction) -> impl AssessmentReader + 'a {
        Reader(tx.connection(), true)
    }
}
impl SqlxAssessmentReaderFactory {
    /// Caller owns a repeatable snapshot and canonical locks in a writable transaction.
    pub fn projection_reader(connection: &mut PgConnection) -> impl AssessmentReader + '_ {
        Reader(connection, false)
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
    at: OffsetDateTime,
) -> Result<AssertionEvidence, AssessmentError> {
    if verified_at > at {
        return Err(corrupt());
    }
    Ok(AssertionEvidence { verified_at })
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
    enabled_methods: Vec<String>,
}
#[derive(FromRow)]
struct ProfileRow {
    #[sqlx(flatten)]
    profile: super::ProfileRow,
    integration_method: Option<String>,
    verified_at: OffsetDateTime,
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
impl ListingRow {
    fn dependency(self, at: OffsetDateTime) -> Result<Option<ListingDependency>, AssessmentError> {
        let row = self;
        match row.lifecycle.as_str() {
            "ACTIVE" => {}
            "WITHDRAWN" => return Ok(None),
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
            DeliveryIntegration::Provider { method } => {
                row.enabled_methods.iter().any(|m| m == method.as_str())
            }
        };
        let assertion = if has_configuration {
            Some(evidence(row.delivery_verified_at.ok_or_else(corrupt)?, at)?)
        } else {
            if row.delivery_verified_at.is_some() {
                return Err(corrupt());
            }
            None
        };
        Ok(Some(ListingDependency {
            id: ProductListingId::try_from(row.product_listing_id).map_err(invalid)?,
            source_id,
            revision: revision(row.version)?,
            source_revision: revision(row.source_revision)?,
            operator_id: PartyId::try_from(row.operator_party_id).map_err(invalid)?,
            delivery,
            evidence: assertion,
            integration_current,
        }))
    }
}

impl ProfileRow {
    fn dependency(self, at: OffsetDateTime) -> Result<ProfileDependency, AssessmentError> {
        let row = self;
        let view = row.profile.view().map_err(invalid)?;
        let integration = match row.integration_method {
            None => DeliveryIntegration::Source,
            Some(method) => shipping_json::listing_delivery::decode_integration(
                serde_json::json!({"kind":"PROVIDER","method":method}),
            )
            .map_err(invalid_box)?,
        };
        Ok(ProfileDependency {
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
            evidence: evidence(row.verified_at, at)?,
        })
    }
}

impl LocationRow {
    fn dependency(
        self,
        resolution: Option<ResolutionView>,
    ) -> Result<LocationDependency, AssessmentError> {
        let row = self;
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
        Ok(LocationDependency {
            valid_until: resolved
                .filter(|r| r.accepted_geography().is_some())
                .and_then(|r| r.valid_until),
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
        })
    }
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
        if self.1 {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                .execute(&mut *self.0)
                .await
                .map_err(storage)?;
        }
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
                l.delivery_verified_at,
                ARRAY(SELECT ingestion_method FROM listing_source_ingestion_methods m WHERE m.listing_source_id=l.listing_source_id) AS enabled_methods
            FROM product_listings l JOIN listing_sources s USING(listing_source_id)
            WHERE l.product_listing_id=ANY($1)
        "#).bind(ids.iter().map(|i| i.into_uuid()).collect::<Vec<_>>()).fetch_all(&mut *self.0).await.map_err(storage)?;
        let mut listings = rows
            .into_iter()
            .filter_map(|row| row.dependency(at).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        apply_import_authority(self.0, &mut listings, at).await?;
        let sources: Vec<_> = listings.iter().map(|l| l.source_id.into_uuid()).collect();
        let default_rows: Vec<(Uuid,Option<Uuid>,i64,bool)> = sqlx::query_as("SELECT d.listing_source_id,CASE WHEN (i.owner_hash IS NOT NULL AND NOT shipping_import_current(d.listing_source_id,i.source_revision)) THEN NULL ELSE d.shipping_profile_id END,d.revision,d.source_deleted FROM shipping_source_defaults d LEFT JOIN shipping_default_import_state i USING(listing_source_id) WHERE d.listing_source_id=ANY($1)").bind(&sources).fetch_all(&mut *self.0).await.map_err(storage)?;
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
                integration_method,verified_at
            FROM shipping_profiles p WHERE shipping_profile_id=ANY($1) AND current_source_id=listing_source_id AND listing_source_id=ANY($2) AND NOT EXISTS(SELECT 1 FROM shipping_profile_import_state i WHERE i.shipping_profile_id=p.shipping_profile_id AND i.owner_hash IS NOT NULL AND NOT shipping_import_current(p.listing_source_id,i.source_revision))
        "#).bind(&profile_ids).bind(&sources).fetch_all(&mut *self.0).await.map_err(storage)?;
        let profiles = rows
            .into_iter()
            .map(|row| row.dependency(at))
            .collect::<Result<Vec<_>, _>>()?;
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
        // Dispatch and collection are independent: a 100-listing page can have
        // 200 sites. Keep bounded shared reads inside this same snapshot.
        let mut resolutions = Vec::with_capacity(subjects.len());
        for batch in subjects.chunks(geo_service::resolution::MAX_RESOLUTION_SUBJECTS) {
            let values = geo_postgres::SqlxResolutionRepositoryFactory::snapshot_reader(self.0)
                .inspect_many(batch)
                .await
                .map_err(resolution_error)?;
            if values.len() != batch.len() {
                return Err(corrupt());
            }
            resolutions.extend(values);
        }
        let locations = rows
            .into_iter()
            .zip(resolutions)
            .map(|(row, resolution)| row.dependency(resolution))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AssessmentDependencies {
            evaluated_at: at,
            listings,
            profiles,
            defaults,
            locations,
        })
    }
}

/// One bounded batch for component observations and authority; no per-listing hydration.
async fn apply_import_authority(
    connection: &mut PgConnection,
    listings: &mut [ListingDependency],
    at: OffsetDateTime,
) -> Result<(), AssessmentError> {
    use shipping_core::{
        inputs::ListingRateInputs,
        selection::{ComponentOverride, DeliveryOverrides},
    };
    #[derive(FromRow)]
    struct Component {
        product_listing_id: Uuid,
        component: String,
        verified_at: Option<OffsetDateTime>,
        current: bool,
    }
    let ids: Vec<_> = listings.iter().map(|l| l.id.into_uuid()).collect();
    // Manual ownership rows without prior imported observations still protect overrides.
    let rows=sqlx::query_as::<_,Component>("SELECT i.product_listing_id,i.component,i.verified_at,(i.owner_hash IS NULL OR shipping_import_current(l.listing_source_id,i.source_revision)) AS current FROM listing_delivery_import_state i JOIN product_listings l USING(product_listing_id) WHERE i.product_listing_id=ANY($1)")
        .bind(&ids).fetch_all(&mut *connection).await.map_err(storage)?;
    for listing in listings {
        let mut selection = listing.delivery.selection().cloned();
        let mut inputs = listing.delivery.rate_inputs().clone();
        let mut shipping = listing.delivery.overrides().shipping.clone();
        let mut collection = listing.delivery.overrides().collection.clone();
        let owned: Vec<_> = rows
            .iter()
            .filter(|r| r.product_listing_id == listing.id.into_uuid())
            .collect();
        // An imported observation can be committed without a semantic listing change,
        // including when SOURCE inheritance has no stored delivery configuration.
        if let Some(verified_at) = owned.iter().filter_map(|r| r.verified_at).min() {
            listing.evidence = Some(AssertionEvidence { verified_at });
        }
        for row in &owned {
            if let Some(verified) = row.verified_at {
                evidence(verified, at)?;
            }
            if !row.current {
                match row.component.as_str() {
                    "SELECTION" => selection = None,
                    "RATE_INPUTS" => inputs = ListingRateInputs::default(),
                    "SHIPPING" => shipping = ComponentOverride::Inherit,
                    "COLLECTION" => collection = ComponentOverride::Inherit,
                    _ => return Err(corrupt()),
                }
            }
        }
        if !owned.is_empty() && !listing.integration_current {
            // Independent manual overrides survive a provider disconnect. Provider membership
            // and selections remain unusable; no source-default fallback is introduced.
            selection = None;
            inputs = ListingRateInputs::default();
            listing.integration_current = true;
        }
        listing.delivery = ListingDelivery::new(
            listing.source_id,
            listing.delivery.integration().clone(),
            selection,
            inputs,
            DeliveryOverrides {
                shipping,
                collection,
            },
        )
        .map_err(invalid)?;
    }
    Ok(())
}
