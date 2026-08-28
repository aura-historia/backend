use super::{SqlxListingSourceReaders, read_error};
use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::ports::{
    ListingSourceDetails, ListingSourceDetailsReader, ListingSourceReadError,
};
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;
use url::Url;

#[derive(sqlx::FromRow)]
struct DetailRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
    operator_party_id: uuid::Uuid,
    party_slug_id: String,
    operator_name: String,
    methods: Vec<String>,
    url: Option<String>,
    image: Option<String>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

fn detail(row: DetailRow) -> Result<ListingSourceDetails, ListingSourceReadError> {
    Ok(ListingSourceDetails {
        listing_source_id: ListingSourceId::from(row.listing_source_id),
        slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        name: ListingSourceName::from(row.name),
        operator_party_id: PartyId::from(row.operator_party_id),
        operator_slug_id: PartySlugId::raw(row.party_slug_id).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        operator_name: PartyName::from(row.operator_name),
        acquisition_methods: row
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

const DETAIL_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.operator_party_id,p.party_slug_id,p.name AS operator_name,array_agg(m.acquisition_method) AS methods,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_id=$1 GROUP BY s.listing_source_id,p.party_id";
const DETAIL_BY_SLUG_SQL: &str = "SELECT s.listing_source_id,s.listing_source_slug_id,s.name,s.operator_party_id,p.party_slug_id,p.name AS operator_name,array_agg(m.acquisition_method) AS methods,s.url,s.image,s.created,s.updated FROM listing_sources s JOIN parties p ON p.party_id=s.operator_party_id LEFT JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE s.listing_source_slug_id=$1 GROUP BY s.listing_source_id,p.party_id";

#[async_trait::async_trait]
impl ListingSourceDetailsReader for SqlxListingSourceReaders {
    async fn find_details_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError> {
        sqlx::query_as::<_, DetailRow>(DETAIL_SQL)
            .bind(uuid::Uuid::from(id))
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
