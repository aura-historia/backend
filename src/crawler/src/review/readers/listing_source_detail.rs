//! `GET /api/listing_sources/{id}`: one ListingSource with its review history and the live URL
//! pattern and product schemas it crawls and scrapes by. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{ListingSourceOverviewReader, ReviewQuery, ReviewQueueReader};
use crate::review::model::{
    ListingSourceDetail, PageSize, ProductSchemaOverview, UrlPatternOverview,
};
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgPool, Row};

/// Reads the page of one ListingSource.
#[derive(Clone)]
pub struct ListingSourceDetailReader {
    pool: PgPool,
    overviews: ListingSourceOverviewReader,
    reviews: ReviewQueueReader,
}

impl ListingSourceDetailReader {
    pub fn new(overviews: ListingSourceOverviewReader, reviews: ReviewQueueReader) -> Self {
        Self {
            pool: overviews.pool().clone(),
            overviews,
            reviews,
        }
    }

    /// The page of `listing_source_id`, with the page of its reviews, newest first, that `reviews`
    /// asks for; `None` when no such ListingSource exists.
    pub async fn find_listing_source(
        &self,
        listing_source_id: ListingSourceId,
        reviews: ReviewHistoryQuery,
    ) -> Result<Option<ListingSourceDetail>, super::PageReadError> {
        let Some(listing_source) = self
            .overviews
            .find_listing_source(listing_source_id)
            .await?
        else {
            return Ok(None);
        };
        let url_pattern = self.url_pattern(listing_source_id).await?;
        let product_schema = self.product_schema(listing_source_id).await?;
        let reviews = self
            .reviews
            .list_reviews(&ReviewQuery {
                listing_source_id: Some(listing_source_id),
                after: reviews.after,
                page_size: reviews.page_size,
                ..ReviewQuery::default()
            })
            .await?;
        Ok(Some(ListingSourceDetail {
            listing_source,
            url_pattern,
            product_schema,
            reviews,
        }))
    }

    async fn url_pattern(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<UrlPatternOverview, sqlx::Error> {
        let row = sqlx::query(
            "SELECT url_pattern_state, url_pattern, url_pattern_exclude
             FROM listing_sources
             WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&self.pool)
        .await?;
        Ok(UrlPatternOverview {
            state: row.try_get("url_pattern_state")?,
            pattern: row.try_get("url_pattern")?,
            exclude: row.try_get("url_pattern_exclude")?,
        })
    }

    /// The live product schemas, decoded as the scraper reads them, so a stored schema that does
    /// not read as one fails the page instead of showing something else.
    async fn product_schema(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ProductSchemaOverview>, sqlx::Error> {
        Ok(ListingSourceProductSchemaRepositoryImpl::new(&self.pool)
            .find_product_schema(&listing_source_id)
            .await?
            .map(|stored| ProductSchemaOverview {
                schemas: stored.product_schemas,
                created: stored.created,
                updated: stored.updated,
            }))
    }
}

/// Which page of a ListingSource's review history to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReviewHistoryQuery {
    pub after: Option<crate::CrawlerReviewId>,
    pub page_size: PageSize,
}
