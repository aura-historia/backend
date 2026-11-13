use super::{SqlxListingSourceReaders, invalid_read, read_error};
use application::error::box_error;
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, PlatformTechnology,
};
use listing_source_service::ports::{
    ListingSourceDetails, ListingSourceDetailsReader, ListingSourceReadError,
};
use partnership_core::partnership_id::PartnershipId;
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;
use url::Url;

#[derive(sqlx::FromRow)]
struct DetailRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
    listing_source_type: String,
    operator_party_id: uuid::Uuid,
    party_slug_id: String,
    operator_name: String,
    operator_partnership_id: Option<uuid::Uuid>,
    methods: Vec<String>,
    platform_technology: Option<String>,
    url: Option<String>,
    image: Option<String>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

fn detail(row: DetailRow) -> Result<ListingSourceDetails, ListingSourceReadError> {
    Ok(ListingSourceDetails {
        listing_source_id: ListingSourceId::try_from(row.listing_source_id)
            .map_err(InvalidDetailObjectId::ListingSource)
            .map_err(invalid_read)?,
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        name: ListingSourceName::try_from(row.name).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        listing_source_type: row.listing_source_type.parse().map_err(invalid_read)?,
        operator_party_id: PartyId::try_from(row.operator_party_id)
            .map_err(InvalidDetailObjectId::OperatorParty)
            .map_err(invalid_read)?,
        operator_slug_id: PartySlugId::raw(row.party_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        operator_name: PartyName::try_from(row.operator_name).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        operator_partnership_id: row
            .operator_partnership_id
            .map(PartnershipId::try_from)
            .transpose()
            .map_err(InvalidDetailObjectId::OperatorPartnership)
            .map_err(invalid_read)?,
        platform_technology: row
            .platform_technology
            .map(PlatformTechnology::try_from)
            .transpose()
            .map_err(invalid_read)?,
        ingestion_methods: row
            .methods
            .into_iter()
            .map(|value| value.parse())
            .collect::<Result<_, _>>()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        url: row
            .url
            .map(|value| Url::parse(&value))
            .transpose()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        image: row
            .image
            .map(|value| Url::parse(&value))
            .transpose()
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?,
        created: row.created,
        updated: row.updated,
    })
}

#[derive(Debug, thiserror::Error)]
enum InvalidDetailObjectId {
    #[error("invalid ListingSource ID persisted")]
    ListingSource(#[source] ObjectIdError),
    #[error("invalid operator Party ID persisted")]
    OperatorParty(#[source] ObjectIdError),
    #[error("invalid operator Partnership ID persisted")]
    OperatorPartnership(#[source] ObjectIdError),
}

const DETAIL_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.listing_source_type,s.operator_party_id,p.party_slug_id,p.name AS operator_name,operator_partnership.partnership_id AS operator_partnership_id,COALESCE(array_agg(m.ingestion_method) FILTER (WHERE m.ingestion_method IS NOT NULL), ARRAY[]::text[]) AS methods,s.platform_technology,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN partnerships operator_partnership ON operator_partnership.party_id=s.operator_party_id AND operator_partnership.business_state='ACTIVE' LEFT JOIN listing_source_ingestion_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_id=$1 GROUP BY s.listing_source_id,p.party_id,operator_partnership.partnership_id";
const DETAIL_BY_SLUG_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.listing_source_type,s.operator_party_id,p.party_slug_id,p.name AS operator_name,operator_partnership.partnership_id AS operator_partnership_id,COALESCE(array_agg(m.ingestion_method) FILTER (WHERE m.ingestion_method IS NOT NULL), ARRAY[]::text[]) AS methods,s.platform_technology,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN partnerships operator_partnership ON operator_partnership.party_id=s.operator_party_id AND operator_partnership.business_state='ACTIVE' LEFT JOIN listing_source_ingestion_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_slug_id=$1 GROUP BY s.listing_source_id,p.party_id,operator_partnership.partnership_id";

#[async_trait::async_trait]
impl ListingSourceDetailsReader for SqlxListingSourceReaders {
    async fn find_details_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError> {
        sqlx::query_as::<_, DetailRow>(DETAIL_SQL)
            .bind(id.into_uuid())
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(detail)
            .transpose()
    }

    async fn find_details_by_slug(
        &self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError> {
        sqlx::query_as::<_, DetailRow>(DETAIL_BY_SLUG_SQL)
            .bind(slug.as_ref())
            .fetch_optional(&self.pool)
            .await
            .map_err(read_error)?
            .map(detail)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn row(listing_source_id: uuid::Uuid, operator_party_id: uuid::Uuid) -> DetailRow {
        DetailRow {
            listing_source_id,
            listing_source_slug_id: "source".to_owned(),
            name: "Source".to_owned(),
            listing_source_type: "DEALER".to_owned(),
            operator_party_id,
            party_slug_id: "operator".to_owned(),
            operator_name: "Operator".to_owned(),
            operator_partnership_id: None,
            methods: Vec::new(),
            platform_technology: None,
            url: None,
            image: None,
            created: datetime!(2026-01-01 00:00 UTC),
            updated: datetime!(2026-01-01 00:00 UTC),
        }
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        assert!(detail(row(uuid::Uuid::new_v4(), uuid::Uuid::now_v7())).is_err());
    }

    #[test]
    fn should_reject_wrong_version_operator_party_id() {
        assert!(detail(row(uuid::Uuid::now_v7(), uuid::Uuid::new_v4())).is_err());
    }

    #[test]
    fn should_reject_wrong_version_operator_partnership_id() {
        let mut invalid = row(uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        invalid.operator_partnership_id = Some(uuid::Uuid::new_v4());
        assert!(detail(invalid).is_err());
    }

    #[test]
    fn should_reject_unknown_persisted_listing_source_type() {
        let mut invalid = row(uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        invalid.listing_source_type = "dealer".to_owned();
        assert!(detail(invalid).is_err());
    }

    #[test]
    fn should_reject_noncanonical_persisted_platform_technology() {
        let mut invalid = row(uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        invalid.platform_technology = Some("shopify".to_owned());
        assert!(detail(invalid).is_err());
    }
}
