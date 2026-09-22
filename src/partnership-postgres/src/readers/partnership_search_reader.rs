use application::{
    error::box_error,
    pagination::{Cursor, CursoredResult},
};
use domain_primitives::object_id::ObjectIdError;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::{
    ports::{PartnershipSearchReadError, PartnershipSearchReader, PartnershipSearchReaderFactory},
    use_cases::queries::list_admin_partnerships::{
        AdminPartnershipSummary, ListAdminPartnershipsRequest, ListAdminPartnershipsResult,
        PartnershipPartySummary, PartnershipSearchCursor,
    },
};
use party_core::{
    party_id::PartyId,
    party_name::{PartyName, PartyNameError},
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use platform_postgres::SqlxTransaction;
use sqlx::{Postgres, QueryBuilder};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipSearchReaderFactory;

struct SqlxPartnershipSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartnershipSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipSearchReaderFactory<SqlxTransaction> for SqlxPartnershipSearchReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnershipSearchReader + 'tx {
        SqlxPartnershipSearchReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct PartnershipSearchRow {
    partnership_id: uuid::Uuid,
    party_id: uuid::Uuid,
    party_slug_id: String,
    party_name: String,
    member_count: i64,
    listing_source_grant_count: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum PartnershipSearchRowMappingError {
    #[error("invalid persisted Partnership ID")]
    PartnershipId(#[source] ObjectIdError),
    #[error("invalid persisted Party ID")]
    PartyId(#[source] ObjectIdError),
    #[error("invalid persisted party slug")]
    PartySlug(#[source] InvalidPartySlugId),
    #[error("invalid persisted party name")]
    PartyName(#[source] PartyNameError),
    #[error("invalid persisted member count")]
    MemberCount(#[source] std::num::TryFromIntError),
    #[error("invalid persisted listing source grant count")]
    ListingSourceGrantCount(#[source] std::num::TryFromIntError),
}

impl TryFrom<PartnershipSearchRow> for AdminPartnershipSummary {
    type Error = PartnershipSearchRowMappingError;

    fn try_from(row: PartnershipSearchRow) -> Result<Self, Self::Error> {
        let member_count = u64::try_from(row.member_count)
            .map_err(PartnershipSearchRowMappingError::MemberCount)?;
        let listing_source_grant_count = u64::try_from(row.listing_source_grant_count)
            .map_err(PartnershipSearchRowMappingError::ListingSourceGrantCount)?;

        Ok(Self {
            partnership_id: PartnershipId::try_from(row.partnership_id)
                .map_err(Self::Error::PartnershipId)?,
            party: PartnershipPartySummary {
                party_id: PartyId::try_from(row.party_id).map_err(Self::Error::PartyId)?,
                party_slug_id: PartySlugId::raw(row.party_slug_id)
                    .map_err(PartnershipSearchRowMappingError::PartySlug)?,
                name: PartyName::try_from(row.party_name)
                    .map_err(PartnershipSearchRowMappingError::PartyName)?,
            },
            member_count,
            listing_source_grant_count,
            created: row.created,
            updated: row.updated,
        })
    }
}

#[async_trait::async_trait]
impl PartnershipSearchReader for SqlxPartnershipSearchReader<'_> {
    async fn search(
        &mut self,
        request: &ListAdminPartnershipsRequest,
    ) -> Result<ListAdminPartnershipsResult, PartnershipSearchReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartnershipSearchReadError::Internal {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(size + 1).map_err(|source| PartnershipSearchReadError::Internal {
                source: box_error(source),
            })?;

        let mut builder = QueryBuilder::<Postgres>::new(
            "WITH candidate_partnerships AS (SELECT p.partnership_id, p.party_id, p.created, p.updated FROM partnerships p WHERE p.business_state = 'ACTIVE'",
        );
        push_filters(&mut builder, request);
        if let Some(search_after) = cursor.search_after {
            builder
                .push(" AND (p.created, p.partnership_id) < (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(search_after.partnership_id.into_uuid())
                .push(")");
        }
        builder
            .push(" ORDER BY p.created DESC, p.partnership_id DESC LIMIT ")
            .push_bind(limit)
            .push(") SELECT candidate.partnership_id, candidate.party_id, party.party_slug_id, party.name AS party_name, (SELECT COUNT(*) FROM partnership_members members WHERE members.partnership_id = candidate.partnership_id) AS member_count, (SELECT COUNT(*) FROM partnership_listing_source_grants grants WHERE grants.partnership_id = candidate.partnership_id) AS listing_source_grant_count, candidate.created, candidate.updated FROM candidate_partnerships candidate JOIN parties party ON party.party_id = candidate.party_id ORDER BY candidate.created DESC, candidate.partnership_id DESC");

        let mut rows = builder
            .build_query_as::<PartnershipSearchRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipSearchReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(AdminPartnershipSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartnershipSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| PartnershipSearchCursor {
                position: item.created,
                partnership_id: item.partnership_id,
            })
        } else {
            None
        };

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &ListAdminPartnershipsRequest) {
    if let Some(party_id) = request.party_id {
        builder
            .push(" AND p.party_id = ")
            .push_bind(party_id.into_uuid());
    }
    if let Some(member_user_id) = request.member_user_id {
        builder.push(
            " AND EXISTS (SELECT 1 FROM partnership_members filter_members WHERE filter_members.partnership_id = p.partnership_id AND filter_members.user_id = ",
        );
        builder.push_bind(member_user_id.into_uuid()).push(")");
    }
    if let Some(listing_source_id) = request.listing_source_id {
        builder.push(
            " AND EXISTS (SELECT 1 FROM partnership_listing_source_grants filter_grants WHERE filter_grants.partnership_id = p.partnership_id AND filter_grants.listing_source_id = ",
        );
        builder.push_bind(listing_source_id.into_uuid()).push(")");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn mapping_row() -> PartnershipSearchRow {
        PartnershipSearchRow {
            partnership_id: uuid::Uuid::now_v7(),
            party_id: uuid::Uuid::now_v7(),
            party_slug_id: "valid-party".to_owned(),
            party_name: "Valid party".to_owned(),
            member_count: 0,
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        }
    }

    #[test]
    fn should_reject_invalid_persisted_party_mapping() {
        let mut row = mapping_row();
        row.party_slug_id = "Not a slug".to_owned();
        let result = AdminPartnershipSummary::try_from(row);

        assert!(matches!(
            result,
            Err(PartnershipSearchRowMappingError::PartySlug(_))
        ));
    }

    #[test]
    fn should_reject_negative_persisted_counts() {
        let mut row = mapping_row();
        row.member_count = -1;
        let result = AdminPartnershipSummary::try_from(row);

        assert!(matches!(
            result,
            Err(PartnershipSearchRowMappingError::MemberCount(_))
        ));
    }

    #[test]
    fn should_reject_wrong_version_partnership_id() {
        let mut row = mapping_row();
        row.partnership_id = uuid::Uuid::new_v4();

        assert!(matches!(
            AdminPartnershipSummary::try_from(row),
            Err(PartnershipSearchRowMappingError::PartnershipId(_))
        ));
    }

    #[test]
    fn should_reject_wrong_version_party_id() {
        let mut row = mapping_row();
        row.party_id = uuid::Uuid::new_v4();

        assert!(matches!(
            AdminPartnershipSummary::try_from(row),
            Err(PartnershipSearchRowMappingError::PartyId(_))
        ));
    }
}
