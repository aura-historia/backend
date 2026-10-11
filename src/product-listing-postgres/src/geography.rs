use application::error::{box_error, static_error};
use geo::{
    GeographicDescription, SpatialPosition, SubdivisionCode, country_from_code,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
    reference::ReferenceRelease,
};
use listing_source_core::location_default::{
    SourceLocationAssignment, SourceLocationEvidence, SourceLocationMeaning,
};
use platform_postgres::SqlxTransaction;
use product_listing_service::geography::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
pub struct SqlxGeographyProjectionStoreFactory;
impl GeographyProjectionStoreFactory<SqlxTransaction> for SqlxGeographyProjectionStoreFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl GeographyProjectionStore + 'tx {
        Store {
            connection: tx.connection(),
        }
    }
}
struct Store<'a> {
    connection: &'a mut PgConnection,
}
fn storage(e: impl std::error::Error + Send + Sync + 'static) -> GeographyError {
    GeographyError::Storage(box_error(e))
}
fn invalid() -> GeographyError {
    GeographyError::Storage(static_error("invalid persisted effective geography"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DimensionData {
    location_id: Uuid,
    basis: String,
    reference_release: String,
    country: Option<String>,
    subdivision: Option<String>,
    position: Option<SpatialPositionData>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeographyData {
    business: Option<DimensionData>,
    inventory: Option<DimensionData>,
}
impl From<&GeographicDimension> for DimensionData {
    fn from(d: &GeographicDimension) -> Self {
        Self {
            location_id: d.location_id.into_uuid(),
            basis: GeographicDimension::BASIS.into(),
            reference_release: d.reference_release.as_str().into(),
            country: d.country.map(|c| c.alpha2().into()),
            subdivision: d.subdivision.as_ref().map(|s| s.as_str().into()),
            position: d.position.map(Into::into),
        }
    }
}
impl From<&EffectiveListingGeography> for GeographyData {
    fn from(g: &EffectiveListingGeography) -> Self {
        Self {
            business: g.business.as_ref().map(Into::into),
            inventory: g.inventory.as_ref().map(Into::into),
        }
    }
}
impl TryFrom<DimensionData> for GeographicDimension {
    type Error = GeographyError;
    fn try_from(d: DimensionData) -> Result<Self, Self::Error> {
        if d.basis != GeographicDimension::BASIS {
            return Err(invalid());
        }
        let release = ReferenceRelease::from_code(&d.reference_release).map_err(storage)?;
        let country = d
            .country
            .as_deref()
            .map(|c| country_from_code(c, release))
            .transpose()
            .map_err(storage)?;
        let subdivision = d
            .subdivision
            .as_deref()
            .map(|s| SubdivisionCode::from_code(s, release))
            .transpose()
            .map_err(storage)?;
        GeographicDescription::new_in_release(release, None, country, subdivision, None)
            .map_err(storage)?;
        if country.is_none() && d.subdivision.is_none() && d.position.is_none() {
            return Err(invalid());
        }
        Ok(Self {
            location_id: d.location_id.try_into().map_err(storage)?,
            reference_release: release,
            country,
            subdivision,
            position: d
                .position
                .map(SpatialPosition::try_from)
                .transpose()
                .map_err(storage)?,
        })
    }
}
pub(crate) fn decode(
    value: serde_json::Value,
) -> Result<EffectiveListingGeography, GeographyError> {
    let d: GeographyData = serde_json::from_value(value).map_err(storage)?;
    Ok(EffectiveListingGeography {
        business: d.business.map(TryInto::try_into).transpose()?,
        inventory: d.inventory.map(TryInto::try_into).transpose()?,
    })
}
#[derive(sqlx::FromRow)]
struct WorkRow {
    work_id: Uuid,
    generation: i64,
    dependency_kind: String,
    dependency_id: String,
    after_source: Option<Uuid>,
    after_listing: Option<Uuid>,
}
#[derive(sqlx::FromRow)]
struct InputRow {
    product_listing_id: Uuid,
    listing_source_id: Uuid,
    version: i64,
    lifecycle: String,
    assignment_revision: Option<i64>,
    selected_location_id: Option<Uuid>,
    party_location_id: Option<Uuid>,
    location_revision: Option<i64>,
    input_revision: Option<i64>,
    location_lifecycle: Option<String>,
    business_evidence: Option<serde_json::Value>,
    inventory_evidence: Option<serde_json::Value>,
    geography: Option<serde_json::Value>,
    position: Option<serde_json::Value>,
}
impl TryFrom<InputRow> for GeographyInput {
    type Error = GeographyError;
    fn try_from(r: InputRow) -> Result<Self, Self::Error> {
        if !matches!(r.lifecycle.as_str(), "ACTIVE" | "WITHDRAWN")
            || r.location_lifecycle
                .as_deref()
                .is_some_and(|v| !matches!(v, "ACTIVE" | "RETIRED"))
            || r.assignment_revision.is_some_and(|revision| revision < 2)
            || r.version <= 0
        {
            return Err(invalid());
        }
        let business = decode_evidence(r.business_evidence)?;
        let inventory = decode_evidence(r.inventory_evidence)?;
        let assignment = match r.selected_location_id {
            Some(id) => {
                if r.assignment_revision.is_none()
                    || r.party_location_id != Some(id)
                    || r.input_revision.is_none_or(|revision| revision <= 0)
                    || r.location_revision < r.input_revision
                    || r.location_lifecycle.is_none()
                {
                    return Err(invalid());
                }
                Some(
                    SourceLocationAssignment::new(
                        id.try_into().map_err(storage)?,
                        business,
                        inventory,
                    )
                    .map_err(storage)?,
                )
            }
            None if business.is_none() && inventory.is_none() => None,
            None => return Err(invalid()),
        };
        let description = r
            .geography
            .map(|value| {
                let data: GeographicDescriptionData =
                    serde_json::from_value(value).map_err(storage)?;
                Option::<GeographicDescription>::try_from(data)
                    .map_err(storage)?
                    .ok_or_else(invalid)
            })
            .transpose()?;
        let position = r
            .position
            .map(|v| serde_json::from_value::<SpatialPositionData>(v).map_err(storage))
            .transpose()?
            .map(SpatialPosition::try_from)
            .transpose()
            .map_err(storage)?;
        Ok(Self {
            listing_id: r.product_listing_id.try_into().map_err(storage)?,
            source_id: r.listing_source_id.try_into().map_err(storage)?,
            listing_version: r.version,
            active: r.lifecycle == "ACTIVE",
            assignment_revision: r.assignment_revision.unwrap_or(1),
            location_id: r
                .party_location_id
                .map(TryInto::try_into)
                .transpose()
                .map_err(storage)?,
            location_revision: r.location_revision,
            input_revision: r.input_revision,
            location_active: r.location_lifecycle.as_deref() == Some("ACTIVE"),
            business: assignment
                .as_ref()
                .is_some_and(|a| a.applies_to(SourceLocationMeaning::AssociatedBusiness)),
            inventory: assignment
                .as_ref()
                .is_some_and(|a| a.applies_to(SourceLocationMeaning::AdvertisedInventory)),
            description,
            position,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    scope: String,
    reference: String,
}
fn decode_evidence(
    value: Option<serde_json::Value>,
) -> Result<Option<SourceLocationEvidence>, GeographyError> {
    value
        .map(|value| {
            let evidence: EvidenceData = serde_json::from_value(value).map_err(storage)?;
            SourceLocationEvidence::new(
                evidence.reference,
                SourceLocationMeaning::from_code(&evidence.scope).ok_or_else(invalid)?,
            )
            .map_err(storage)
        })
        .transpose()
}
#[async_trait::async_trait]
impl GeographyProjectionStore for Store<'_> {
    async fn claim(
        &mut self,
        work: Option<GeographyWorkId>,
    ) -> Result<Option<GeographyClaim>, GeographyError> {
        let row=sqlx::query_as::<_,WorkRow>("SELECT * FROM listing_geography_work WHERE pending AND (lease_until IS NULL OR lease_until<=clock_timestamp()) AND ($1::uuid IS NULL OR work_id=$1) ORDER BY pending_since,work_id LIMIT 1 FOR UPDATE SKIP LOCKED")
            .bind(work.map(|w|w.into_uuid())).fetch_optional(&mut *self.connection).await.map_err(storage)?;
        let Some(r) = row else {
            if let Some(work) = work {
                let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_geography_work WHERE work_id=$1 AND pending)").bind(work.into_uuid()).fetch_one(&mut *self.connection).await.map_err(storage)?;
                if pending {
                    return Err(GeographyError::Deferred);
                }
            }
            return Ok(None);
        };
        let token = Uuid::new_v4();
        sqlx::query("UPDATE listing_geography_work SET lease_token=$2,lease_until=clock_timestamp()+interval '60 seconds' WHERE work_id=$1").bind(r.work_id).bind(token).execute(&mut *self.connection).await.map_err(storage)?;
        let age: Option<f64>=sqlx::query_scalar("SELECT EXTRACT(epoch FROM clock_timestamp()-min(pending_since))::float8 FROM listing_geography_work WHERE pending").fetch_one(&mut *self.connection).await.map_err(storage)?;
        tracing::info!(
            metric = "listing_geography_backlog",
            oldest_pending_seconds = age.unwrap_or(0.0)
        );
        Ok(Some(GeographyClaim {
            work_id: r.work_id.try_into().map_err(storage)?,
            generation: r.generation,
            token,
            kind: r.dependency_kind,
            dependency_id: r.dependency_id,
            after_source: r.after_source,
            after_listing: r.after_listing,
        }))
    }
    async fn page(
        &mut self,
        claim: &GeographyClaim,
        limit: u16,
    ) -> Result<GeographyPage, GeographyError> {
        let selection = match claim.kind.as_str() {
            "SOURCE" => {
                "SELECT listing_source_id FROM listing_sources WHERE listing_source_id=$1::uuid"
            }
            "LOCATION" => {
                "SELECT listing_source_id FROM listing_source_location_defaults WHERE party_location_id=$1::uuid"
            }
            "RESOLUTION" => {
                "SELECT d.listing_source_id FROM geo_resolution_bindings b JOIN listing_source_location_defaults d ON d.party_location_id=b.subject_id WHERE b.subject_kind='PARTY_LOCATION' AND b.resolution_id=$1::uuid"
            }
            "PROFILE" => {
                "SELECT d.listing_source_id FROM geo_resolutions r JOIN geo_resolution_bindings b USING(resolution_id) JOIN listing_source_location_defaults d ON d.party_location_id=b.subject_id WHERE b.subject_kind='PARTY_LOCATION' AND r.profile=$1"
            }
            "LISTING" => {
                "SELECT listing_source_id FROM product_listings WHERE product_listing_id=$1::uuid"
            }
            _ => return Err(invalid()),
        };
        let listing_clause = if claim.kind == "LISTING" {
            "AND p.product_listing_id=$1::uuid"
        } else {
            ""
        };
        let query = format!(
            "WITH sources AS ({selection}) SELECT p.product_listing_id,p.listing_source_id FROM sources s JOIN product_listings p USING(listing_source_id) WHERE ($2::uuid IS NULL OR (p.listing_source_id,p.product_listing_id)>($2,$3)) {listing_clause} ORDER BY p.listing_source_id,p.product_listing_id LIMIT $4"
        );
        let ids: Vec<(Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(query.as_str()))
            .bind(&claim.dependency_id)
            .bind(claim.after_source)
            .bind(claim.after_listing)
            .bind(i64::from(limit))
            .fetch_all(&mut *self.connection)
            .await
            .map_err(storage)?;
        if ids.is_empty() {
            return Ok(GeographyPage {
                complete: true,
                ..Default::default()
            });
        }
        let listing_ids = ids.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let source_ids = ids.iter().map(|(_, id)| *id).collect::<Vec<_>>();
        // Owner -> source -> selected location -> listing, shared with management writes.
        sqlx::query("SELECT party_id FROM parties WHERE party_id IN (SELECT operator_party_id FROM listing_sources WHERE listing_source_id=ANY($1)) ORDER BY party_id FOR SHARE")
            .bind(&source_ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT listing_source_id FROM listing_sources WHERE listing_source_id=ANY($1) ORDER BY listing_source_id FOR SHARE")
            .bind(&source_ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        sqlx::query("SELECT l.party_location_id FROM party_locations l WHERE l.party_location_id IN (SELECT party_location_id FROM listing_source_location_defaults WHERE listing_source_id=ANY($1)) ORDER BY l.party_location_id FOR SHARE")
            .bind(&source_ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let rows=sqlx::query_as::<_,InputRow>(r#"
            SELECT p.product_listing_id,p.listing_source_id,p.version,p.lifecycle,d.revision AS assignment_revision,d.party_location_id AS selected_location_id,
                l.party_location_id,l.revision AS location_revision,l.input_revision,l.lifecycle AS location_lifecycle,
                d.business_evidence,d.inventory_evidence,l.geography,l.position
            FROM product_listings p JOIN listing_sources s USING(listing_source_id)
            LEFT JOIN listing_source_location_defaults d ON d.listing_source_id=s.listing_source_id
            LEFT JOIN party_locations l ON l.party_location_id=d.party_location_id AND l.party_id=s.operator_party_id
            WHERE p.product_listing_id=ANY($1) ORDER BY p.product_listing_id FOR UPDATE OF p
        "#).bind(listing_ids).fetch_all(&mut *self.connection).await.map_err(storage)?;
        let last = ids.last().ok_or_else(invalid)?;
        Ok(GeographyPage {
            inputs: rows
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            after_source: Some(last.1),
            after_listing: Some(last.0),
            complete: ids.len() < usize::from(limit),
        })
    }
    async fn apply(&mut self, writes: &[GeographyWrite]) -> Result<u32, GeographyError> {
        let mut changed = 0;
        for w in writes {
            let i = &w.input;
            let valid_until = w
                .resolution
                .as_ref()
                .filter(|r| {
                    r.accepted_geography().is_some()
                        && (w.geography.business.is_some() || w.geography.inventory.is_some())
                })
                .and_then(|r| r.valid_until);
            let dependency = json!({"policy":POLICY,"listingVersion":i.listing_version,"listingActive":i.active,"sourceId":i.source_id,"assignmentRevision":i.assignment_revision,"locationId":i.location_id,"locationRevision":i.location_revision,"inputRevision":i.input_revision,"locationActive":i.location_active,"business":i.business,"inventory":i.inventory,"resolution":w.resolution.as_ref().map(|r|json!({"id":r.work.id,"generation":r.work.generation,"guard":r.guard,"inputRevision":r.input_revision,"selection":r.selected_candidate,"validUntil":r.valid_until.map(|v|v.unix_timestamp_nanos().to_string()),"policy":geo_service::resolution::POLICY_VERSION}))});
            let geography =
                serde_json::to_value(GeographyData::from(&w.geography)).map_err(storage)?;
            let old:Option<(serde_json::Value,i64,i64,serde_json::Value,bool)>=sqlx::query_as("SELECT geography,geography_version,indexed_version,dependency,blocks_queries FROM product_listing_geographies WHERE product_listing_id=$1").bind(i.listing_id.into_uuid()).fetch_optional(&mut *self.connection).await.map_err(storage)?;
            let empty =
                serde_json::to_value(GeographyData::from(&EffectiveListingGeography::default()))
                    .map_err(storage)?;
            let differs = old.as_ref().map(|o| &o.0).unwrap_or(&empty) != &geography;
            // Adding previously absent geography cannot expose a forbidden stale
            // point. Replacing/removing old geography retains query custody until
            // the new document or tombstone is refresh-visible.
            let replaces_existing = differs && old.as_ref().is_some_and(|old| old.0 != empty);
            let blocks_queries = replaces_existing || old.as_ref().is_some_and(|old| old.4);
            // Dependency equality is a replay guard, never an ordered target version.
            // Compare the safe result too: time-dependent eligibility can change
            // without a new dependency generation.
            if old
                .as_ref()
                .is_some_and(|old| old.3 == dependency && old.0 == geography)
            {
                continue;
            }
            if replaces_existing {
                // Ordinary catch-up can discover retention expiry before the
                // maintenance journal exists. Fence requests that span this
                // removal even if indexing clears custody before their recheck.
                sqlx::query("UPDATE listing_geography_gate SET epoch=epoch+1")
                    .execute(&mut *self.connection)
                    .await
                    .map_err(storage)?;
            }
            let mut version:i64=sqlx::query_scalar("SELECT projection_version FROM product_listings WHERE product_listing_id=$1 AND version=$2").bind(i.listing_id.into_uuid()).bind(i.listing_version).fetch_optional(&mut *self.connection).await.map_err(storage)?.ok_or(GeographyError::Superseded)?;
            if differs {
                let event = domain_primitives::event_id::EventId::new();
                // Enrichment advances only the search fence/current event, not canonical version, timestamps or history.
                sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'ENRICHMENT_GEOGRAPHY_UPDATED','ENRICHMENT',1,'{}',now())").bind(event.into_uuid()).bind(i.listing_id.into_uuid()).execute(&mut *self.connection).await.map_err(storage)?;
                version=sqlx::query_scalar("UPDATE product_listings SET current_event_id=$2,projection_version=projection_version+1 WHERE product_listing_id=$1 RETURNING projection_version").bind(i.listing_id.into_uuid()).bind(event.into_uuid()).fetch_one(&mut *self.connection).await.map_err(storage)?;
                changed += 1;
            } else if let Some(old) = &old {
                version = old.1;
            }
            sqlx::query("INSERT INTO product_listing_geographies(product_listing_id,dependency,geography,valid_until,geography_version,indexed_version,blocks_queries) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(product_listing_id) DO UPDATE SET dependency=EXCLUDED.dependency,geography=EXCLUDED.geography,valid_until=EXCLUDED.valid_until,geography_version=EXCLUDED.geography_version,blocks_queries=EXCLUDED.blocks_queries")
                .bind(i.listing_id.into_uuid()).bind(dependency).bind(geography).bind(valid_until).bind(version).bind(old.map(|o|o.2).unwrap_or(if differs {0} else {version})).bind(blocks_queries).execute(&mut *self.connection).await.map_err(storage)?;
        }
        Ok(changed)
    }
    async fn checkpoint(
        &mut self,
        claim: &GeographyClaim,
        page: &GeographyPage,
    ) -> Result<(), GeographyError> {
        let updated=sqlx::query("UPDATE listing_geography_work SET after_source=$4,after_listing=$5,pending=$6,generation=generation+1,lease_token=NULL,lease_until=NULL WHERE work_id=$1 AND generation=$2 AND lease_token=$3 AND lease_until>clock_timestamp()")
            .bind(claim.work_id.into_uuid()).bind(claim.generation).bind(claim.token).bind(page.after_source).bind(page.after_listing).bind(!page.complete).execute(&mut *self.connection).await.map_err(storage)?;
        if updated.rows_affected() != 1 {
            return Err(GeographyError::Superseded);
        }
        if !page.complete {
            sqlx::query("INSERT INTO listing_geography_wakeups(work_id,generation) VALUES($1,$2)")
                .bind(claim.work_id.into_uuid())
                .bind(claim.generation + 1)
                .execute(&mut *self.connection)
                .await
                .map_err(storage)?;
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct SqlxGeographicQueryGate {
    pool: PgPool,
}
impl SqlxGeographicQueryGate {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl GeographicQueryGate for SqlxGeographicQueryGate {
    async fn permit(&self) -> Result<Option<GeographicQueryPermit>, GeographyError> {
        let epoch=sqlx::query_scalar::<_,i64>(r#"
            SELECT epoch FROM listing_geography_gate WHERE NOT EXISTS(SELECT 1 FROM listing_geography_work WHERE pending AND blocks_queries)
            AND NOT EXISTS(SELECT 1 FROM product_listing_geographies WHERE blocks_queries AND indexed_version<geography_version)
            AND NOT EXISTS(SELECT 1 FROM product_listing_geographies WHERE valid_until<=clock_timestamp())
        "#).fetch_optional(&self.pool).await.map_err(storage)?;
        Ok(epoch.map(|epoch| GeographicQueryPermit { epoch }))
    }
}

#[derive(Clone)]
pub struct SqlxEffectiveListingGeographyBatchReader {
    pool: PgPool,
}
impl SqlxEffectiveListingGeographyBatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl EffectiveListingGeographyBatchReader for SqlxEffectiveListingGeographyBatchReader {
    async fn read_many(
        &self,
        ids: &[product_listing_core::product_listing_id::ProductListingId],
    ) -> Result<
        std::collections::HashMap<
            product_listing_core::product_listing_id::ProductListingId,
            EffectiveListingGeography,
        >,
        GeographyError,
    > {
        if ids.len() > 100 {
            return Err(GeographyError::InvalidInput);
        }
        let ids = ids.iter().map(|id| id.into_uuid()).collect::<Vec<_>>();
        let gate = SqlxGeographicQueryGate::new(self.pool.clone());
        Ok(with_geographic_eligibility(&gate,|| async {
            let rows:Vec<(Uuid,serde_json::Value)>=sqlx::query_as("SELECT g.product_listing_id,g.geography FROM product_listing_geographies g JOIN product_listings p USING(product_listing_id) WHERE g.product_listing_id=ANY($1) AND p.lifecycle='ACTIVE'")
                .bind(ids).fetch_all(&self.pool).await.map_err(storage)?;
            rows.into_iter().map(|(id,value)|Ok((id.try_into().map_err(storage)?,decode(value)?))).collect()
        }).await?.unwrap_or_default())
    }
}
