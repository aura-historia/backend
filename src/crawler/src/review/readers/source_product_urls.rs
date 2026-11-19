//! The product URLs of a product-schema review's ListingSource that `GET /api/reviews/{id}`
//! offers next to the review's own pages. The contract is in `docs/crawler/operations.md`
//! (review agents).

use crate::CrawlerReviewId;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, SourceProductUrl};
use crate::scraper::css_selector::product_schema::decode_stored_product_schemas;
use crate::scraper::css_selector::rule::ExtractionRule;
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
            "SELECT su.url, su.last_scraped, su.last_error_kind,
                    su.last_source_listing_id, su.last_extracted_source_listing_id
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
                captured_identity: row.try_get("last_source_listing_id")?,
                page_identity: row.try_get("last_extracted_source_listing_id")?,
            })
        })
        .collect()
    }

    /// The `source_listing_id` rule of each live product schema of the review's ListingSource,
    /// which its listings were captured under, in the order the schemas are stored; none for a
    /// URL-pattern review, an unknown review and a ListingSource without live schemas. A stored
    /// schema list that does not decode fails the read.
    pub async fn live_identity_rules(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<Option<ExtractionRule>>, sqlx::Error> {
        let stored: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT s.product_schema
             FROM crawler_reviews r
             JOIN listing_source_product_schemas s ON s.listing_source_id = r.listing_source_id
             WHERE r.review_id = $1
               AND r.artifact_type = $2",
        )
        .bind(review_id.as_uuid())
        .bind(ARTIFACT_PRODUCT_SCHEMA)
        .fetch_optional(&self.pool)
        .await?;
        let Some(stored) = stored else {
            return Ok(Vec::new());
        };
        Ok(decode_stored_product_schemas(stored)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?
            .into_iter()
            .map(|schema| schema.source_listing_id)
            .collect())
    }
}
