use application::{error::box_error, operation_context::Principal};
use geo::{
    DerivedGeography, GeographicDescription, SpatialPosition,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
};
use listing_source_core::{ListingSourceId, location_default::*};
use listing_source_service::{ports::location_default::*, use_cases::location_default::*};
use party_core::{
    party_id::PartyId,
    party_location::{PartyLocationDisclosure, PartyLocationLifecycle},
    party_location_id::PartyLocationId,
};
use platform_postgres::SqlxTransaction;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxSourceLocationAccessFactory;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxLocationDefaultRepositoryFactory;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxLocationDefaultReaderFactory;
struct Access<'a>(&'a mut PgConnection);
struct Repository<'a>(&'a mut PgConnection);
struct Reader<'a>(&'a mut PgConnection);
impl SourceLocationAccessFactory<SqlxTransaction> for SqlxSourceLocationAccessFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SourceLocationAccess + 'tx {
        Access(tx.connection())
    }
}
impl LocationDefaultRepositoryFactory<SqlxTransaction> for SqlxLocationDefaultRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl LocationDefaultRepository + 'tx {
        Repository(tx.connection())
    }
}
impl LocationDefaultReaderFactory<SqlxTransaction> for SqlxLocationDefaultReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl LocationDefaultReader + 'tx {
        Reader(tx.connection())
    }
}
fn sql_error(source: sqlx::Error) -> LocationDefaultError {
    if let sqlx::Error::Database(ref e) = source
        && (e.is_check_violation() || e.is_foreign_key_violation() || e.is_unique_violation())
    {
        return LocationDefaultError::Internal {
            source: box_error(source),
        };
    }
    LocationDefaultError::TemporarilyUnavailable {
        source: box_error(source),
    }
}
fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> LocationDefaultError {
    LocationDefaultError::InvalidPersistedState {
        source: box_error(source),
    }
}
fn corrupt() -> LocationDefaultError {
    invalid(std::io::Error::other(
        "invalid source location default encoding",
    ))
}
fn eligibility(value: &str) -> Result<SelectedLocationEligibility, LocationDefaultError> {
    match PartyLocationLifecycle::from_code(value).ok_or_else(corrupt)? {
        PartyLocationLifecycle::Active => Ok(SelectedLocationEligibility::Active),
        PartyLocationLifecycle::Retired => Ok(SelectedLocationEligibility::Retired),
    }
}
#[async_trait::async_trait]
impl SourceLocationAccess for Access<'_> {
    async fn authorize_source(
        &mut self,
        principal: &Principal,
        id: ListingSourceId,
        policy: SourceLocationAccessPolicy,
    ) -> Result<PartyId, LocationDefaultError> {
        // Same Party-first lock order as Party location/partnership mutations. Never lock a source first.
        let owner: Uuid = sqlx::query_scalar(
            "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
        )
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or(LocationDefaultError::SourceNotFound)?;
        let party_id = PartyId::try_from(owner).map_err(invalid)?;
        sqlx::query_scalar::<_, Uuid>("SELECT party_id FROM parties WHERE party_id=$1 FOR UPDATE")
            .bind(owner)
            .fetch_optional(&mut *self.0)
            .await
            .map_err(sql_error)?
            .ok_or(LocationDefaultError::SourceNotFound)?;
        let current: Uuid = sqlx::query_scalar(
            "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1 FOR UPDATE",
        )
        .bind(id.into_uuid())
        .fetch_optional(&mut *self.0)
        .await
        .map_err(sql_error)?
        .ok_or(LocationDefaultError::SourceNotFound)?;
        if owner != current {
            return Err(LocationDefaultError::ConcurrencyConflict);
        }
        let user_id = match principal {
            Principal::Service(_) | Principal::System => return Ok(party_id),
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => id,
            Principal::Anonymous => return Err(LocationDefaultError::AuthenticationRequired),
        };
        // Hold user authority against suspension/admin-role changes through commit.
        let user: Option<(String, bool)> =
            sqlx::query_as("SELECT role,suspended FROM users WHERE user_id=$1 FOR SHARE")
                .bind(user_id.into_uuid())
                .fetch_optional(&mut *self.0)
                .await
                .map_err(sql_error)?;
        let (role, suspended) = user.ok_or(LocationDefaultError::Forbidden)?;
        if suspended {
            return Err(LocationDefaultError::Forbidden);
        }
        let role = user_core::role::UserRole::from_code(&role).ok_or_else(corrupt)?;
        if role == user_core::role::UserRole::Admin {
            return Ok(party_id);
        }
        if policy == SourceLocationAccessPolicy::AdministratorOnly {
            return Err(LocationDefaultError::Forbidden);
        }
        // Party partnership membership also confers PartyLocation management; source grant is additional.
        let granted = sqlx::query_scalar::<_, Uuid>("SELECT p.partnership_id FROM partnership_members m JOIN partnerships p ON p.partnership_id=m.partnership_id JOIN partnership_listing_source_grants g ON g.partnership_id=p.partnership_id WHERE m.user_id=$1 AND p.party_id=$2 AND p.business_state='ACTIVE' AND g.listing_source_id=$3 FOR SHARE OF p,m,g")
            .bind(user_id.into_uuid()).bind(owner).bind(id.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
        if granted.is_some() {
            Ok(party_id)
        } else {
            Err(LocationDefaultError::Forbidden)
        }
    }
    async fn check_location(
        &mut self,
        party_id: PartyId,
        location_id: PartyLocationId,
    ) -> Result<SelectedLocationEligibility, LocationDefaultError> {
        let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM party_locations WHERE party_location_id=$1 AND party_id=$2 FOR SHARE")
            .bind(location_id.into_uuid()).bind(party_id.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(LocationDefaultError::LocationNotFound)?;
        eligibility(&lifecycle)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    scope: String,
}
fn encode_evidence(
    evidence: Option<&SourceLocationEvidence>,
) -> Result<Option<serde_json::Value>, LocationDefaultError> {
    evidence
        .map(|e| {
            serde_json::to_value(EvidenceData {
                reference: e.reference().to_owned(),
                scope: e.scope().as_str().to_owned(),
            })
            .map_err(|source| LocationDefaultError::Internal {
                source: box_error(source),
            })
        })
        .transpose()
}
fn decode_evidence(
    value: Option<serde_json::Value>,
) -> Result<Option<SourceLocationEvidence>, LocationDefaultError> {
    value
        .map(|v| {
            let e: EvidenceData = serde_json::from_value(v).map_err(invalid)?;
            SourceLocationEvidence::new(
                e.reference,
                SourceLocationMeaning::from_code(&e.scope).ok_or_else(corrupt)?,
            )
            .map_err(invalid)
        })
        .transpose()
}
#[derive(FromRow)]
struct DefaultRow {
    listing_source_id: Uuid,
    party_location_id: Option<Uuid>,
    business_evidence: Option<serde_json::Value>,
    inventory_evidence: Option<serde_json::Value>,
    revision: i64,
}
impl TryFrom<DefaultRow> for StoredLocationDefault {
    type Error = LocationDefaultError;
    fn try_from(row: DefaultRow) -> Result<Self, Self::Error> {
        if row.revision < 2 {
            return Err(corrupt());
        }
        let business = decode_evidence(row.business_evidence)?;
        let inventory = decode_evidence(row.inventory_evidence)?;
        let assignment = match row.party_location_id {
            Some(id) => Some(
                SourceLocationAssignment::new(
                    PartyLocationId::try_from(id).map_err(invalid)?,
                    business,
                    inventory,
                )
                .map_err(invalid)?,
            ),
            None if business.is_none() && inventory.is_none() => None,
            None => return Err(corrupt()),
        };
        Ok(Self {
            default: ListingSourceLocationDefault::rehydrate(
                ListingSourceId::try_from(row.listing_source_id).map_err(invalid)?,
                assignment,
            ),
            revision: LocationDefaultRevision::try_from(row.revision).map_err(invalid)?,
        })
    }
}
#[async_trait::async_trait]
impl LocationDefaultRepository for Repository<'_> {
    async fn find_for_update(
        &mut self,
        id: ListingSourceId,
    ) -> Result<StoredLocationDefault, LocationDefaultError> {
        let row = sqlx::query_as::<_, DefaultRow>("SELECT listing_source_id,party_location_id,business_evidence,inventory_evidence,revision FROM listing_source_location_defaults WHERE listing_source_id=$1 FOR UPDATE")
            .bind(id.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?;
        match row {
            Some(row) => row.try_into(),
            None => Ok(StoredLocationDefault {
                default: ListingSourceLocationDefault::empty(id),
                revision: LocationDefaultRevision::INITIAL,
            }),
        }
    }
    async fn update(
        &mut self,
        stored: &StoredLocationDefault,
        actor: &str,
    ) -> Result<StoredLocationDefault, LocationDefaultError> {
        if stored.revision.into_inner() >= i64::MAX as u64 {
            return Err(LocationDefaultError::RevisionExhausted);
        }
        let assignment = stored.default.assignment();
        let old_revision = i64::try_from(stored.revision.into_inner()).map_err(invalid)?;
        let row = sqlx::query_as::<_, DefaultRow>(
            "INSERT INTO listing_source_location_defaults(listing_source_id,party_id,party_location_id,business_evidence,inventory_evidence,revision,updated_actor) SELECT listing_source_id,operator_party_id,$2,$3,$4,$5+1,$6 FROM listing_sources WHERE listing_source_id=$1 AND ($5=1 OR EXISTS(SELECT 1 FROM listing_source_location_defaults WHERE listing_source_id=$1 AND revision=$5)) ON CONFLICT(listing_source_id) DO UPDATE SET party_location_id=EXCLUDED.party_location_id,business_evidence=EXCLUDED.business_evidence,inventory_evidence=EXCLUDED.inventory_evidence,revision=EXCLUDED.revision,updated_actor=EXCLUDED.updated_actor,updated=now() WHERE listing_source_location_defaults.revision=$5 RETURNING listing_source_id,party_location_id,business_evidence,inventory_evidence,revision")
            .bind(stored.default.source_id().into_uuid()).bind(assignment.map(|a| a.location_id().into_uuid()))
            .bind(encode_evidence(assignment.and_then(|a| a.business_association()))?).bind(encode_evidence(assignment.and_then(|a| a.advertised_inventory()))?)
            .bind(old_revision).bind(actor).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(LocationDefaultError::ConcurrencyConflict)?;
        let result: StoredLocationDefault = row.try_into()?;
        // Pure invalidation basis. Clear uses null selection; history survives subsequent replacements/deletion.
        sqlx::query("INSERT INTO listing_source_location_default_changes(listing_source_id,revision,party_id,party_location_id,location_revision,business_association,advertised_inventory,actor) SELECT d.listing_source_id,d.revision,d.party_id,d.party_location_id,l.revision,d.business_evidence IS NOT NULL,d.inventory_evidence IS NOT NULL,$2 FROM listing_source_location_defaults d LEFT JOIN party_locations l ON l.party_location_id=d.party_location_id AND l.party_id=d.party_id WHERE d.listing_source_id=$1")
            .bind(result.default.source_id().into_uuid()).bind(actor).execute(&mut *self.0).await.map_err(sql_error)?;
        Ok(result)
    }
}
#[derive(FromRow)]
struct ManagementRow {
    listing_source_id: Uuid,
    party_location_id: Option<Uuid>,
    business_evidence: Option<serde_json::Value>,
    inventory_evidence: Option<serde_json::Value>,
    revision: Option<i64>,
    lifecycle: Option<String>,
}
#[async_trait::async_trait]
impl LocationDefaultReader for Reader<'_> {
    async fn get(
        &mut self,
        id: ListingSourceId,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        let row = sqlx::query_as::<_, ManagementRow>("SELECT s.listing_source_id,d.party_location_id,d.business_evidence,d.inventory_evidence,d.revision,l.lifecycle FROM listing_sources s LEFT JOIN listing_source_location_defaults d ON d.listing_source_id=s.listing_source_id LEFT JOIN party_locations l ON l.party_location_id=d.party_location_id AND l.party_id=s.operator_party_id WHERE s.listing_source_id=$1")
            .bind(id.into_uuid()).fetch_optional(&mut *self.0).await.map_err(sql_error)?.ok_or(LocationDefaultError::SourceNotFound)?;
        let eligibility = match (row.party_location_id, row.lifecycle) {
            (None, None) => None,
            (Some(_), Some(lifecycle)) => Some(eligibility(&lifecycle)?),
            _ => return Err(corrupt()),
        };
        let stored = match row.revision {
            None if row.party_location_id.is_none()
                && row.business_evidence.is_none()
                && row.inventory_evidence.is_none() =>
            {
                StoredLocationDefault {
                    default: ListingSourceLocationDefault::empty(id),
                    revision: LocationDefaultRevision::INITIAL,
                }
            }
            None => return Err(corrupt()),
            Some(revision) => DefaultRow {
                listing_source_id: row.listing_source_id,
                party_location_id: row.party_location_id,
                business_evidence: row.business_evidence,
                inventory_evidence: row.inventory_evidence,
                revision,
            }
            .try_into()?,
        };
        Ok(LocationDefaultView {
            source_id: stored.default.source_id(),
            assignment: stored.default.assignment().cloned(),
            revision: stored.revision,
            eligibility,
        })
    }
}

#[derive(Clone)]
pub struct SqlxEffectiveSourceGeographyReader {
    pool: PgPool,
}
impl SqlxEffectiveSourceGeographyReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[derive(FromRow)]
struct GeographyRow {
    listing_source_id: Uuid,
    party_location_id: Option<Uuid>,
    business_evidence: Option<serde_json::Value>,
    inventory_evidence: Option<serde_json::Value>,
    revision: Option<i64>,
    location_revision: Option<i64>,
    input_revision: Option<i64>,
    lifecycle: Option<String>,
    disclosure: Option<String>,
    geography: Option<serde_json::Value>,
    position: Option<serde_json::Value>,
}
fn map_geography(row: GeographyRow) -> Result<EffectiveSourceGeographyView, LocationDefaultError> {
    let source_id = ListingSourceId::try_from(row.listing_source_id).map_err(invalid)?;
    let Some(revision) = row.revision else {
        if row.party_location_id.is_some()
            || row.business_evidence.is_some()
            || row.inventory_evidence.is_some()
        {
            return Err(corrupt());
        }
        return Ok(EffectiveSourceGeographyView::default());
    };
    let stored: StoredLocationDefault = DefaultRow {
        listing_source_id: row.listing_source_id,
        party_location_id: row.party_location_id,
        business_evidence: row.business_evidence,
        inventory_evidence: row.inventory_evidence,
        revision,
    }
    .try_into()?;
    let Some(assignment) = stored.default.assignment() else {
        return Ok(EffectiveSourceGeographyView::default());
    };
    let lifecycle = eligibility(row.lifecycle.as_deref().ok_or_else(corrupt)?)?;
    let disclosure =
        PartyLocationDisclosure::from_code(row.disclosure.as_deref().ok_or_else(corrupt)?)
            .ok_or_else(corrupt)?;
    let location_revision = row.location_revision.ok_or_else(corrupt)?;
    let input_revision = row.input_revision.ok_or_else(corrupt)?;
    if input_revision <= 0 || location_revision < input_revision {
        return Err(corrupt());
    }
    if lifecycle != SelectedLocationEligibility::Active
        || disclosure == PartyLocationDisclosure::Private
    {
        return Ok(EffectiveSourceGeographyView::default());
    }
    let description = row
        .geography
        .map(|value| {
            let data: GeographicDescriptionData = serde_json::from_value(value).map_err(invalid)?;
            Option::<GeographicDescription>::try_from(data)
                .map_err(invalid)?
                .ok_or_else(corrupt)
        })
        .transpose()?;
    let position = row
        .position
        .map(|value| {
            let data: SpatialPositionData = serde_json::from_value(value).map_err(invalid)?;
            SpatialPosition::try_from(data).map_err(invalid)
        })
        .transpose()?;
    let (description, position) = match disclosure {
        PartyLocationDisclosure::CoarsePublic => (
            description
                .map(|d| {
                    GeographicDescription::new_in_release(
                        d.reference_release(),
                        None,
                        d.country(),
                        d.subdivision(),
                        None,
                    )
                    .map_err(invalid)
                })
                .transpose()?
                .flatten(),
            None,
        ),
        PartyLocationDisclosure::ExactPublic => (description, position),
        PartyLocationDisclosure::Private => unreachable!(),
    };
    let geography = DerivedGeography::new(description, position);
    let view = |meaning| SourceDefaultGeographyView {
        basis: SourceDefaultGeographyBasis {
            source_id,
            assignment_revision: stored.revision,
            location_id: assignment.location_id(),
            location_revision: location_revision as u64,
            location_input_revision: input_revision as u64,
            meaning,
        },
        geography: geography.clone(),
    };
    Ok(EffectiveSourceGeographyView {
        business_association: assignment
            .applies_to(SourceLocationMeaning::AssociatedBusiness)
            .then(|| view(SourceLocationMeaning::AssociatedBusiness)),
        advertised_inventory: assignment
            .applies_to(SourceLocationMeaning::AdvertisedInventory)
            .then(|| view(SourceLocationMeaning::AdvertisedInventory)),
    })
}
#[async_trait::async_trait]
impl EffectiveSourceGeographyReader for SqlxEffectiveSourceGeographyReader {
    async fn get(
        &self,
        id: ListingSourceId,
    ) -> Result<EffectiveSourceGeographyView, LocationDefaultError> {
        let row = sqlx::query_as::<_, GeographyRow>("SELECT s.listing_source_id,d.party_location_id,d.business_evidence,d.inventory_evidence,d.revision,l.revision AS location_revision,l.input_revision,l.lifecycle,l.disclosure,CASE WHEN l.lifecycle='ACTIVE' AND l.disclosure IN ('COARSE_PUBLIC','EXACT_PUBLIC') THEN l.geography END AS geography,CASE WHEN l.lifecycle='ACTIVE' AND l.disclosure='EXACT_PUBLIC' THEN l.position END AS position FROM listing_sources s LEFT JOIN listing_source_location_defaults d ON d.listing_source_id=s.listing_source_id LEFT JOIN party_locations l ON l.party_location_id=d.party_location_id AND l.party_id=s.operator_party_id WHERE s.listing_source_id=$1")
            .bind(id.into_uuid()).fetch_optional(&self.pool).await.map_err(sql_error)?.ok_or(LocationDefaultError::SourceNotFound)?;
        map_geography(row)
    }
}
