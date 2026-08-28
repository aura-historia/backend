use super::{SqlxListingSourceReaders, read_error};
use application::error::box_error;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{ListingSourceReadError, WebCrawlSource, WebCrawlSourceReader};
use party_core::party_id::PartyId;
use url::Url;

#[async_trait::async_trait]
impl WebCrawlSourceReader for SqlxListingSourceReaders {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError> {
        let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, String)>("SELECT s.listing_source_id,s.operator_party_id,s.url FROM listing_sources s JOIN listing_source_acquisition_methods m ON m.listing_source_id=s.listing_source_id WHERE m.acquisition_method='WEB_CRAWL' AND s.url IS NOT NULL")
            .fetch_all(&self.pool).await.map_err(read_error)?;
        rows.into_iter()
            .map(|(id, party, url)| {
                Ok(WebCrawlSource {
                    listing_source_id: ListingSourceId::from(id),
                    operator_party_id: PartyId::from(party),
                    url: Url::parse(&url).map_err(|error| {
                        ListingSourceReadError::InvalidReadModel {
                            source: box_error(error),
                        }
                    })?,
                })
            })
            .collect()
    }
}
