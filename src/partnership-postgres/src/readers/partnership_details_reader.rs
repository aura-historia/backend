use application::error::box_error;
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_service::{
    ports::{
        PartnershipDetailsReadError, PartnershipDetailsReader, PartnershipDetailsReaderFactory,
    },
    use_cases::queries::{
        get_admin_partnership::AdminPartnershipDetailsView,
        list_admin_partnerships::PartnershipPartySummary,
    },
};
use party_core::{
    party_id::PartyId,
    party_name::{PartyName, PartyNameError},
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use platform_postgres::SqlxTransaction;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartnershipDetailsReaderFactory;

struct SqlxPartnershipDetailsReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartnershipDetailsReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartnershipDetailsReaderFactory<SqlxTransaction> for SqlxPartnershipDetailsReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartnershipDetailsReader + 'tx {
        SqlxPartnershipDetailsReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct PartnershipDetailsRow {
    partnership_id: Uuid,
    party_id: Uuid,
    party_slug_id: String,
    party_name: String,
    member_user_ids: Vec<Uuid>,
    member_count: i64,
    listing_source_ids: Vec<Uuid>,
    listing_source_grant_count: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
enum PartnershipDetailsRowMappingError {
    #[error("invalid persisted Partnership ID")]
    PartnershipId(#[source] ObjectIdError),
    #[error("invalid persisted Party ID")]
    PartyId(#[source] ObjectIdError),
    #[error("invalid persisted member User ID")]
    MemberUserId(#[source] ObjectIdError),
    #[error("invalid persisted ListingSource ID")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid persisted party slug")]
    PartySlug(#[source] InvalidPartySlugId),
    #[error("invalid persisted party name")]
    PartyName(#[source] PartyNameError),
    #[error("invalid persisted member count")]
    MemberCount(#[source] std::num::TryFromIntError),
    #[error("invalid persisted listing source grant count")]
    ListingSourceGrantCount(#[source] std::num::TryFromIntError),
}

impl TryFrom<PartnershipDetailsRow> for AdminPartnershipDetailsView {
    type Error = PartnershipDetailsRowMappingError;

    fn try_from(row: PartnershipDetailsRow) -> Result<Self, Self::Error> {
        let member_count = u64::try_from(row.member_count)
            .map_err(PartnershipDetailsRowMappingError::MemberCount)?;
        let listing_source_grant_count = u64::try_from(row.listing_source_grant_count)
            .map_err(PartnershipDetailsRowMappingError::ListingSourceGrantCount)?;

        Ok(Self {
            partnership_id: PartnershipId::try_from(row.partnership_id)
                .map_err(Self::Error::PartnershipId)?,
            party: PartnershipPartySummary {
                party_id: PartyId::try_from(row.party_id).map_err(Self::Error::PartyId)?,
                party_slug_id: PartySlugId::raw(row.party_slug_id)
                    .map_err(PartnershipDetailsRowMappingError::PartySlug)?,
                name: PartyName::try_from(row.party_name)
                    .map_err(PartnershipDetailsRowMappingError::PartyName)?,
            },
            member_user_ids: row
                .member_user_ids
                .into_iter()
                .map(|id| UserId::try_from(id).map_err(Self::Error::MemberUserId))
                .collect::<Result<_, _>>()?,
            listing_source_ids: row
                .listing_source_ids
                .into_iter()
                .map(|id| ListingSourceId::try_from(id).map_err(Self::Error::ListingSourceId))
                .collect::<Result<_, _>>()?,
            member_count,
            listing_source_grant_count,
            created: row.created,
            updated: row.updated,
        })
    }
}

const DETAILS_SQL: &str = "SELECT p.partnership_id,p.party_id,party.party_slug_id,party.name AS party_name,ARRAY(SELECT members.user_id FROM partnership_members members WHERE members.partnership_id=p.partnership_id ORDER BY members.user_id LIMIT 100) AS member_user_ids,(SELECT COUNT(*) FROM partnership_members members WHERE members.partnership_id=p.partnership_id) AS member_count,ARRAY(SELECT grants.listing_source_id FROM partnership_listing_source_grants grants WHERE grants.partnership_id=p.partnership_id ORDER BY grants.listing_source_id LIMIT 100) AS listing_source_ids,(SELECT COUNT(*) FROM partnership_listing_source_grants grants WHERE grants.partnership_id=p.partnership_id) AS listing_source_grant_count,p.created,p.updated FROM partnerships p JOIN parties party ON party.party_id=p.party_id WHERE p.partnership_id=$1";

#[async_trait::async_trait]
impl PartnershipDetailsReader for SqlxPartnershipDetailsReader<'_> {
    async fn find_by_id(
        &mut self,
        partnership_id: PartnershipId,
    ) -> Result<Option<AdminPartnershipDetailsView>, PartnershipDetailsReadError> {
        sqlx::query_as::<_, PartnershipDetailsRow>(DETAILS_SQL)
            .bind(partnership_id.into_uuid())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(
                |source| PartnershipDetailsReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?
            .map(AdminPartnershipDetailsView::try_from)
            .transpose()
            .map_err(|source| PartnershipDetailsReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn mapping_row() -> PartnershipDetailsRow {
        PartnershipDetailsRow {
            partnership_id: Uuid::now_v7(),
            party_id: Uuid::now_v7(),
            party_slug_id: "valid-party".to_owned(),
            party_name: "Valid party".to_owned(),
            member_user_ids: Vec::new(),
            member_count: 0,
            listing_source_ids: Vec::new(),
            listing_source_grant_count: 0,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        }
    }

    #[test]
    fn should_reject_wrong_version_persisted_object_ids() {
        let mut partnership = mapping_row();
        partnership.partnership_id = Uuid::new_v4();
        assert!(matches!(
            AdminPartnershipDetailsView::try_from(partnership),
            Err(PartnershipDetailsRowMappingError::PartnershipId(_))
        ));

        let mut party = mapping_row();
        party.party_id = Uuid::new_v4();
        assert!(matches!(
            AdminPartnershipDetailsView::try_from(party),
            Err(PartnershipDetailsRowMappingError::PartyId(_))
        ));

        let mut member = mapping_row();
        member.member_user_ids.push(Uuid::new_v4());
        assert!(matches!(
            AdminPartnershipDetailsView::try_from(member),
            Err(PartnershipDetailsRowMappingError::MemberUserId(_))
        ));

        let mut listing_source = mapping_row();
        listing_source.listing_source_ids.push(Uuid::new_v4());
        assert!(matches!(
            AdminPartnershipDetailsView::try_from(listing_source),
            Err(PartnershipDetailsRowMappingError::ListingSourceId(_))
        ));
    }

    #[test]
    fn should_reject_invalid_persisted_party_mapping() {
        let mut row = mapping_row();
        row.party_slug_id = "Not a slug".to_owned();

        assert!(matches!(
            AdminPartnershipDetailsView::try_from(row),
            Err(PartnershipDetailsRowMappingError::PartySlug(_))
        ));
    }

    #[test]
    fn should_reject_negative_persisted_counts() {
        let mut row = mapping_row();
        row.member_count = -1;

        assert!(matches!(
            AdminPartnershipDetailsView::try_from(row),
            Err(PartnershipDetailsRowMappingError::MemberCount(_))
        ));
    }
}
