use super::{SqlxListingSourceReaders, read_error};
use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::ports::{ListingSourceReadError, WebCrawlSource, WebCrawlSourceReader};

#[async_trait::async_trait]
impl WebCrawlSourceReader for SqlxListingSourceReaders {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError> {
        let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, bool)>(
            "SELECT s.listing_source_id, s.name, s.listing_source_slug_id, \
                    EXISTS ( \
                        SELECT 1 FROM listing_source_ingestion_methods m \
                        WHERE m.listing_source_id = s.listing_source_id \
                          AND m.ingestion_method = 'WEB_CRAWL' \
                    ) AS web_crawl_enabled \
             FROM listing_sources s",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(read_error)?;

        rows.into_iter()
            .map(|(id, name, slug, web_crawl_enabled)| {
                Ok(WebCrawlSource {
                    listing_source_id: ListingSourceId::from(id),
                    listing_source_name: ListingSourceName::try_from(name).map_err(|error| {
                        ListingSourceReadError::InvalidReadModel {
                            source: box_error(error),
                        }
                    })?,
                    listing_source_slug: ListingSourceSlugId::raw(slug).map_err(|error| {
                        ListingSourceReadError::InvalidReadModel {
                            source: box_error(error),
                        }
                    })?,
                    web_crawl_enabled,
                })
            })
            .collect()
    }
}
