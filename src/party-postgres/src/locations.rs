use application::error::box_error;
use geo::{
    GeographicDescription, SpatialPosition,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
};
use party_core::{
    party_id::PartyId,
    party_location::{
        PartyLocation, PartyLocationContent, PartyLocationInputRevision, PartyLocationLabel,
        PartyLocationLifecycle, PartyLocationRole,
    },
    party_location_id::PartyLocationId,
};
use party_service::{ports::party_location::*, use_cases::party_locations::*};
use platform_postgres::SqlxTransaction;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection, Postgres, QueryBuilder};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use user_core::role::UserRole;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationRepositoryFactory;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationAccessFactory;
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartyLocationReaderFactory;
impl geo_service::resolution::ResolutionRepositoryFactory<SqlxTransaction>
    for SqlxPartyLocationRepositoryFactory
{
    fn resolutions<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl geo_service::resolution::ResolutionRepository + 'tx {
        geo_postgres::SqlxResolutionRepositoryFactory::on_connection(tx.connection())
    }
    fn resolution_reader<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl geo_service::resolution::ResolutionReader + 'tx {
        geo_postgres::SqlxResolutionRepositoryFactory::on_connection(tx.connection())
    }
}
struct LocationRepository<'a> {
    connection: &'a mut PgConnection,
}
struct LocationAccess<'a> {
    connection: &'a mut PgConnection,
}
struct LocationReader<'a> {
    connection: &'a mut PgConnection,
}
impl PartyLocationRepositoryFactory<SqlxTransaction> for SqlxPartyLocationRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationRepository + 'tx {
        LocationRepository {
            connection: tx.connection(),
        }
    }
}
impl PartyLocationAccessFactory<SqlxTransaction> for SqlxPartyLocationAccessFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationAccess + 'tx {
        LocationAccess {
            connection: tx.connection(),
        }
    }
}
impl PartyLocationReaderFactory<SqlxTransaction> for SqlxPartyLocationReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartyLocationReader + 'tx {
        LocationReader {
            connection: tx.connection(),
        }
    }
}
fn sql_error(source: sqlx::Error) -> PartyLocationError {
    if let sqlx::Error::Database(ref e) = source
        && (e.is_foreign_key_violation() || e.is_unique_violation() || e.is_check_violation())
    {
        return PartyLocationError::Internal {
            source: box_error(source),
        };
    }
    PartyLocationError::TemporarilyUnavailable {
        source: box_error(source),
    }
}
fn invalid(source: impl std::error::Error + Send + Sync + 'static) -> PartyLocationError {
    PartyLocationError::InvalidPersistedState {
        source: box_error(source),
    }
}
fn corrupt() -> PartyLocationError {
    invalid(std::io::Error::other(
        "invalid persisted party location encoding",
    ))
}
fn encode<T: Serialize>(value: &T) -> Result<serde_json::Value, PartyLocationError> {
    serde_json::to_value(value).map_err(|source| PartyLocationError::Internal {
        source: box_error(source),
    })
}
fn decode<T: serde::de::DeserializeOwned>(
    value: serde_json::Value,
) -> Result<T, PartyLocationError> {
    serde_json::from_value(value).map_err(invalid)
}
fn geography(
    value: Option<GeographicDescriptionData>,
) -> Result<Option<GeographicDescription>, PartyLocationError> {
    value
        .map(|v| {
            Option::<GeographicDescription>::try_from(v)
                .map_err(invalid)?
                .ok_or_else(corrupt)
        })
        .transpose()
}
fn label(value: String) -> Result<PartyLocationLabel, PartyLocationError> {
    let label = PartyLocationLabel::new(value.clone()).map_err(invalid)?;
    if label.as_str() != value {
        return Err(corrupt());
    }
    Ok(label)
}
fn roles(values: Vec<String>) -> Result<BTreeSet<PartyLocationRole>, PartyLocationError> {
    let count = values.len();
    let set = values
        .into_iter()
        .map(|v| PartyLocationRole::from_code(&v).ok_or_else(corrupt))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if count != set.len() {
        return Err(corrupt());
    }
    Ok(set)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    #[serde(with = "time::serde::rfc3339::option")]
    observed_at: Option<OffsetDateTime>,
    scope: String,
}
impl From<&LocationEvidence> for EvidenceData {
    fn from(value: &LocationEvidence) -> Self {
        Self {
            reference: value.reference().to_owned(),
            observed_at: value.observed_at(),
            scope: value.scope().as_str().to_owned(),
        }
    }
}
impl TryFrom<EvidenceData> for LocationEvidence {
    type Error = PartyLocationError;
    fn try_from(value: EvidenceData) -> Result<Self, Self::Error> {
        Self::new(
            value.reference,
            value.observed_at,
            LocationAssertionScope::from_code(&value.scope).ok_or_else(corrupt)?,
        )
        .map_err(invalid)
    }
}
struct ContentData {
    label: String,
    roles: Vec<String>,
    geography: Option<GeographicDescriptionData>,
    position: Option<SpatialPositionData>,
}
impl From<&PartyLocationContent> for ContentData {
    fn from(c: &PartyLocationContent) -> Self {
        Self {
            label: c.label.as_str().to_owned(),
            roles: c.roles.iter().map(|r| r.as_str().to_owned()).collect(),
            geography: c.geography.as_ref().map(Into::into),
            position: c.position.map(Into::into),
        }
    }
}
#[derive(FromRow)]
struct LocationRow {
    party_location_id: Uuid,
    party_id: Uuid,
    label: String,
    roles: Vec<String>,
    geography: Option<serde_json::Value>,
    position: Option<serde_json::Value>,
    lifecycle: String,
    revision: i64,
    input_revision: i64,
    evidence: Option<serde_json::Value>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}
impl TryFrom<LocationRow> for StoredPartyLocation {
    type Error = PartyLocationError;
    fn try_from(r: LocationRow) -> Result<Self, Self::Error> {
        let revision = PartyLocationRevision::try_from(r.revision).map_err(invalid)?;
        let input_revision =
            PartyLocationInputRevision::try_from(r.input_revision).map_err(invalid)?;
        if input_revision.into_inner() > revision.into_inner() {
            return Err(corrupt());
        }
        Ok(Self {
            location: PartyLocation::rehydrate(
                PartyLocationId::try_from(r.party_location_id).map_err(invalid)?,
                PartyId::try_from(r.party_id).map_err(invalid)?,
                PartyLocationContent {
                    label: label(r.label)?,
                    roles: roles(r.roles)?,
                    geography: geography(r.geography.map(decode).transpose()?)?,
                    position: r
                        .position
                        .map(decode::<SpatialPositionData>)
                        .transpose()?
                        .map(SpatialPosition::try_from)
                        .transpose()
                        .map_err(invalid)?,
                },
                PartyLocationLifecycle::from_code(&r.lifecycle).ok_or_else(corrupt)?,
                input_revision,
            ),
            revision,
            evidence: r
                .evidence
                .map(decode::<EvidenceData>)
                .transpose()?
                .map(LocationEvidence::try_from)
                .transpose()?,
            created: r.created,
            updated: r.updated,
        })
    }
}
const COLUMNS: &str = "party_location_id, party_id, label, roles, geography, position, lifecycle, revision, input_revision, evidence, created, updated";

impl LocationAccess<'_> {
    async fn lock_active_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserRole>, PartyLocationError> {
        // The Party is locked first. Hold user authority against demotion,
        // suspension and account deletion until the protected operation commits.
        let user: Option<(String, bool)> =
            sqlx::query_as("SELECT role,suspended FROM users WHERE user_id=$1 FOR SHARE")
                .bind(user_id.into_uuid())
                .fetch_optional(&mut *self.connection)
                .await
                .map_err(sql_error)?;
        match user {
            Some((role, false)) => Ok(Some(UserRole::from_code(&role).ok_or_else(corrupt)?)),
            _ => Ok(None),
        }
    }
}

#[async_trait::async_trait]
impl PartyLocationAccess for LocationAccess<'_> {
    async fn lock_party(&mut self, party_id: PartyId) -> Result<bool, PartyLocationError> {
        Ok(sqlx::query_scalar::<_, Uuid>(
            "SELECT party_id FROM parties WHERE party_id = $1 FOR UPDATE",
        )
        .bind(party_id.into_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql_error)?
        .is_some())
    }
    async fn can_administer_locations(
        &mut self,
        user_id: UserId,
    ) -> Result<bool, PartyLocationError> {
        Ok(self.lock_active_user(user_id).await? == Some(UserRole::Admin))
    }
    async fn can_manage_locations(
        &mut self,
        party_id: PartyId,
        user_id: UserId,
    ) -> Result<bool, PartyLocationError> {
        if self.lock_active_user(user_id).await?.is_none() {
            return Ok(false);
        }
        sqlx::query_scalar(
            "SELECT EXISTS ( \
                SELECT 1 FROM users u WHERE u.user_id = $2 AND NOT u.suspended AND ( \
                    EXISTS (SELECT 1 FROM party_location_management_grants g \
                            WHERE g.party_id = $1 AND g.user_id = u.user_id) \
                    OR EXISTS (SELECT 1 FROM partnership_members m \
                               JOIN partnerships p ON p.partnership_id = m.partnership_id \
                               WHERE m.user_id = u.user_id AND p.party_id = $1 \
                                 AND p.business_state = 'ACTIVE') \
                ) \
            )",
        )
        .bind(party_id.into_uuid())
        .bind(user_id.into_uuid())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql_error)
    }
    async fn set_management_grant(
        &mut self,
        party_id: PartyId,
        user_id: UserId,
        granted: bool,
        actor: &str,
    ) -> Result<(), PartyLocationError> {
        if granted {
            let user = sqlx::query_scalar::<_, Uuid>(
                "SELECT user_id FROM users WHERE user_id=$1 FOR KEY SHARE",
            )
            .bind(user_id.into_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(sql_error)?;
            if user.is_none() {
                return Err(PartyLocationError::NotFound);
            }
            sqlx::query("INSERT INTO party_location_management_grants(party_id, user_id, granted_actor) VALUES ($1,$2,$3) ON CONFLICT (party_id,user_id) DO NOTHING").bind(party_id.into_uuid()).bind(user_id.into_uuid()).bind(actor).execute(&mut *self.connection).await.map_err(sql_error)?;
        } else {
            sqlx::query(
                "DELETE FROM party_location_management_grants WHERE party_id = $1 AND user_id = $2",
            )
            .bind(party_id.into_uuid())
            .bind(user_id.into_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(sql_error)?;
        }
        Ok(())
    }
}
impl LocationRepository<'_> {
    async fn signal(
        &mut self,
        result: &StoredPartyLocation,
        actor: &str,
    ) -> Result<(), PartyLocationError> {
        sqlx::query("INSERT INTO party_location_changes(party_location_id,revision,party_id,input_revision,lifecycle,actor) VALUES ($1,$2,$3,$4,$5,$6)").bind(result.location.id().into_uuid()).bind(i64::try_from(result.revision.into_inner()).map_err(invalid)?).bind(result.location.party_id().into_uuid()).bind(i64::try_from(result.location.input_revision().into_inner()).map_err(invalid)?).bind(result.location.lifecycle().as_str()).bind(actor).execute(&mut *self.connection).await.map_err(sql_error)?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl PartyLocationRepository for LocationRepository<'_> {
    async fn find_for_update(
        &mut self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<Option<StoredPartyLocation>, PartyLocationError> {
        let mut q = QueryBuilder::<Postgres>::new("SELECT ");
        q.push(COLUMNS)
            .push(" FROM party_locations WHERE party_id = ")
            .push_bind(party_id.into_uuid())
            .push(" AND party_location_id = ")
            .push_bind(id.into_uuid())
            .push(" FOR UPDATE");
        q.build_query_as::<LocationRow>()
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .map(TryInto::try_into)
            .transpose()
    }
    async fn insert(
        &mut self,
        location: &PartyLocation,
        evidence: Option<&LocationEvidence>,
        actor: &str,
    ) -> Result<StoredPartyLocation, PartyLocationError> {
        let c = ContentData::from(location.content());
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO party_locations (party_location_id,party_id,label,roles,geography,position,lifecycle,input_revision,evidence,created_actor,updated_actor) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10) RETURNING ",
        );
        q.push(COLUMNS);
        let result: StoredPartyLocation = q
            .build_query_as::<LocationRow>()
            .bind(location.id().into_uuid())
            .bind(location.party_id().into_uuid())
            .bind(c.label)
            .bind(c.roles)
            .bind(c.geography.map(|v| encode(&v)).transpose()?)
            .bind(c.position.map(|v| encode(&v)).transpose()?)
            .bind(location.lifecycle().as_str())
            .bind(i64::try_from(location.input_revision().into_inner()).map_err(invalid)?)
            .bind(
                evidence
                    .map(|v| encode(&EvidenceData::from(v)))
                    .transpose()?,
            )
            .bind(actor)
            .fetch_one(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .try_into()?;
        self.signal(&result, actor).await?;
        Ok(result)
    }
    async fn update(
        &mut self,
        stored: &StoredPartyLocation,
        actor: &str,
    ) -> Result<StoredPartyLocation, PartyLocationError> {
        if stored.revision.into_inner() >= i64::MAX as u64 {
            return Err(PartyLocationError::RevisionExhausted);
        }
        let c = ContentData::from(stored.location.content());
        let mut q = QueryBuilder::<Postgres>::new(
            "UPDATE party_locations SET label=$1,roles=$2,geography=$3,position=$4,lifecycle=$5,input_revision=$6,evidence=$7,updated_actor=$8,revision=revision+1,updated=aura_advance_updated(updated) WHERE party_location_id=$9 AND party_id=$10 AND revision=$11 RETURNING ",
        );
        q.push(COLUMNS);
        let result: StoredPartyLocation = q
            .build_query_as::<LocationRow>()
            .bind(c.label)
            .bind(c.roles)
            .bind(c.geography.map(|v| encode(&v)).transpose()?)
            .bind(c.position.map(|v| encode(&v)).transpose()?)
            .bind(stored.location.lifecycle().as_str())
            .bind(i64::try_from(stored.location.input_revision().into_inner()).map_err(invalid)?)
            .bind(
                stored
                    .evidence
                    .as_ref()
                    .map(|v| encode(&EvidenceData::from(v)))
                    .transpose()?,
            )
            .bind(actor)
            .bind(stored.location.id().into_uuid())
            .bind(stored.location.party_id().into_uuid())
            .bind(i64::try_from(stored.revision.into_inner()).map_err(invalid)?)
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .ok_or(PartyLocationError::ConcurrencyConflict)?
            .try_into()?;
        self.signal(&result, actor).await?;
        Ok(result)
    }
}

/// Storage representation for protected management reads.
#[derive(FromRow)]
struct ViewRow {
    party_location_id: Uuid,
    party_id: Uuid,
    label: String,
    roles: Vec<String>,
    geography: Option<serde_json::Value>,
    position: Option<serde_json::Value>,
    lifecycle: String,
    revision: i64,
    input_revision: i64,
    evidence: Option<serde_json::Value>,
}
impl TryFrom<ViewRow> for PartyLocationView {
    type Error = PartyLocationError;
    fn try_from(r: ViewRow) -> Result<Self, Self::Error> {
        if r.input_revision > r.revision {
            return Err(corrupt());
        }
        Ok(Self {
            id: PartyLocationId::try_from(r.party_location_id).map_err(invalid)?,
            party_id: PartyId::try_from(r.party_id).map_err(invalid)?,
            label: label(r.label)?,
            roles: roles(r.roles)?,
            geography: geography(r.geography.map(decode).transpose()?)?,
            position: r
                .position
                .map(decode::<SpatialPositionData>)
                .transpose()?
                .map(SpatialPosition::try_from)
                .transpose()
                .map_err(invalid)?,
            lifecycle: PartyLocationLifecycle::from_code(&r.lifecycle).ok_or_else(corrupt)?,
            revision: PartyLocationRevision::try_from(r.revision).map_err(invalid)?,
            input_revision: PartyLocationInputRevision::try_from(r.input_revision)
                .map_err(invalid)?
                .into_inner(),
            evidence: r
                .evidence
                .map(decode::<EvidenceData>)
                .transpose()?
                .map(TryInto::try_into)
                .transpose()?,
        })
    }
}
fn view_query() -> QueryBuilder<Postgres> {
    QueryBuilder::new(
        "SELECT party_location_id,party_id,label,roles,geography,position,lifecycle,revision,input_revision,evidence FROM party_locations WHERE TRUE",
    )
}
#[async_trait::async_trait]
impl PartyLocationReader for LocationReader<'_> {
    async fn get(
        &mut self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<Option<PartyLocationView>, PartyLocationError> {
        let mut q = view_query();
        q.push(" AND party_id = ")
            .push_bind(party_id.into_uuid())
            .push(" AND party_location_id = ")
            .push_bind(id.into_uuid());
        q.build_query_as::<ViewRow>()
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .map(TryInto::try_into)
            .transpose()
    }
    async fn list(
        &mut self,
        party_id: PartyId,
        after: Option<PartyLocationId>,
        limit: u32,
    ) -> Result<PartyLocationsPage, PartyLocationError> {
        if !(1..=100).contains(&limit) {
            return Err(PartyLocationError::InvalidInput);
        }
        let mut q = view_query();
        q.push(" AND party_id = ").push_bind(party_id.into_uuid());
        if let Some(id) = after {
            q.push(" AND party_location_id > ")
                .push_bind(id.into_uuid());
        }
        q.push(" ORDER BY party_location_id ASC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let mut items = q
            .build_query_as::<ViewRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(sql_error)?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<PartyLocationView>, _>>()?;
        let next = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items.last().map(|i| i.id)
        } else {
            None
        };
        Ok(PartyLocationsPage { items, next })
    }
}
