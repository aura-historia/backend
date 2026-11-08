//! The product URLs of a product-schema review's ListingSource that `GET /api/reviews/{id}`
//! offers next to the review's own pages. The contract is in `docs/crawler/operations.md`
//! (review agents).

use crate::CrawlerReviewId;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, SourceProductUrl};
use sqlx::{PgPool, Row};

/// The most URLs offered to one review.
pub const MAX_SOURCE_PRODUCT_URLS: i64 = 40;

/// Reads the product URLs the crawler stored for a review's ListingSource.
#[derive(Clone)]
pub struct SourceProductUrlReader {
    pool: PgPool,
}

impl SourceProductUrlReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Up to [`MAX_SOURCE_PRODUCT_URLS`] active or dormant-sold product URLs of the review's
    /// ListingSource that are not pages of the review. URLs that scraped without an error come
    /// first, the most recently scraped first, then the others by the same rule, ties broken by
    /// URL. A URL-pattern review, which tests no schema, and an unknown review get none.
    pub async fn list(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<SourceProductUrl>, sqlx::Error> {
        sqlx::query(
            "SELECT su.url, su.last_scraped, su.last_error_kind
             FROM crawler_reviews r
             JOIN listing_source_urls su ON su.listing_source_id = r.listing_source_id
             WHERE r.review_id = $1
               AND r.artifact_type = $2
               AND su.url_class = 'product'
               AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND NOT EXISTS (
                   SELECT 1 FROM crawler_review_pages p
                   WHERE p.review_id = r.review_id AND p.url = su.url
               )
             ORDER BY (su.last_scraped IS NOT NULL AND su.last_error_kind IS NULL) DESC,
                      su.last_scraped DESC NULLS LAST,
                      su.url
             LIMIT $3",
        )
        .bind(review_id.as_uuid())
        .bind(ARTIFACT_PRODUCT_SCHEMA)
        .bind(MAX_SOURCE_PRODUCT_URLS)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(SourceProductUrl {
                url: row.try_get("url")?,
                last_scraped: row.try_get("last_scraped")?,
                last_error_kind: row.try_get("last_error_kind")?,
            })
        })
        .collect()
    }
}
