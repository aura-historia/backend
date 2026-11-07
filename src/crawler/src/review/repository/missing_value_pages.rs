//! Pages whose last scrape recorded `MissingRequiredValue`, for a reviewer to declare a
//! removed-page template on (`docs/crawler/extraction-and-review.md`, removed-page templates).
//! Such a page opens and joins no review on its own, so the reviewer opens one for it.

use super::{
    CrawlerReviewRepository, ReviewRepositoryError, SchemaReviewWithStatusInput,
    parse_product_schema_payload, review_source_url_pattern, reviewer_page_url,
};
use crate::CrawlerReviewId;
use crate::review::model::STATUS_PENDING_REVIEW;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;
use listing_source_core::ListingSourceId;
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

/// Why the crawler opened a product-schema review a reviewer asked for on a page that recorded
/// `MissingRequiredValue`. It is no `manual_schema_required` review, so rejecting it classifies
/// no page as a non-product page.
pub const MISSING_REQUIRED_VALUE_REVIEW_REASON: &str = "missing_required_value";

/// The most pages [`CrawlerReviewRepository::list_missing_value_pages`] lists.
pub const MAX_LISTED_MISSING_VALUE_PAGES: i64 = 100;

/// A product page whose last scrape left a required value empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissingValuePage {
    pub url: String,
    /// The outcome message the scrape recorded, which names the blank required values, such as
    /// "Required value missing from '…': `title` is blank".
    pub missing: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_scraped: Option<OffsetDateTime>,
}

impl CrawlerReviewRepository {
    /// The ListingSource's product pages whose last scrape recorded `MissingRequiredValue`,
    /// most recently scraped first, at most [`MAX_LISTED_MISSING_VALUE_PAGES`].
    pub async fn list_missing_value_pages(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<MissingValuePage>, ReviewRepositoryError> {
        let rows = sqlx::query_as::<_, (String, Option<String>, Option<OffsetDateTime>)>(
            "SELECT url, last_error_message, last_scraped FROM listing_source_urls \
             WHERE listing_source_id = $1 AND url_class = 'product' \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
               AND last_error_kind = 'MissingRequiredValue' \
             ORDER BY last_scraped DESC NULLS LAST, url \
             LIMIT $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(MAX_LISTED_MISSING_VALUE_PAGES)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(url, missing, last_scraped)| MissingValuePage {
                url,
                missing,
                last_scraped,
            })
            .collect())
    }

    /// The live schema list a review for `url` starts from. Refuses a URL that is no product
    /// page of the ListingSource whose last scrape recorded `MissingRequiredValue`, and a
    /// ListingSource without live schemas, whose pages need schema work rather than a template.
    /// The console checks it before fetching the page, so it never fetches another URL.
    pub async fn missing_value_page_review_schemas(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
        let recorded: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM listing_source_urls \
             WHERE listing_source_id = $1 AND url = $2 AND url_class = 'product' \
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') \
               AND last_error_kind = 'MissingRequiredValue')",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .fetch_one(&self.pool)
        .await?;
        if !recorded {
            return Err(ReviewRepositoryError::NotMissingValuePage(url.to_string()));
        }
        self.live_product_schemas(listing_source_id).await
    }

    /// The ListingSource's live product schemas, refusing one without any.
    async fn live_product_schemas(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT product_schema FROM listing_source_product_schemas WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .fetch_optional(&self.pool)
        .await?
        .map(parse_product_schema_payload)
        .transpose()?
        .filter(|schemas| !schemas.is_empty())
        .ok_or(ReviewRepositoryError::NoLiveProductSchemas(*listing_source_id))
    }

    /// Refuses a page fetched from `requested_url` that the review it would join refuses as a
    /// reviewer-added page: one served from a URL off the ListingSource's crawl host, or one the
    /// shop redirected to a page that is no product page, such as a sold listing sent to its
    /// category. Checked before the review is opened, so a page it would refuse opens none.
    pub async fn check_missing_value_page_fetch(
        &self,
        listing_source_id: &ListingSourceId,
        requested_url: &Url,
        fetched_url: &Url,
    ) -> Result<(), ReviewRepositoryError> {
        let url = reviewer_page_url(fetched_url);
        let product_url_pattern =
            review_source_url_pattern(&self.pool, uuid::Uuid::from(*listing_source_id), &url)
                .await?;
        if is_redirect_to_non_product_page(requested_url, &url, product_url_pattern.as_deref()) {
            return Err(ReviewRepositoryError::RedirectedToNonProductPage(
                url.to_string(),
            ));
        }
        Ok(())
    }

    /// The pending product-schema review of the ListingSource a reviewer adds `url`, a page
    /// [`Self::missing_value_page_review_schemas`] admits, to: the one already pending, or a new
    /// one whose candidate is the live schema list and the stored removed-page templates.
    pub async fn open_review_for_missing_value_page(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
        let live = self
            .missing_value_page_review_schemas(listing_source_id, url)
            .await?;
        self.insert_schema_review(SchemaReviewWithStatusInput {
            listing_source_id,
            reason: MISSING_REQUIRED_VALUE_REVIEW_REASON,
            schemas: &live,
            pages: Vec::new(),
            validation_summary: serde_json::json!({}),
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
    }
}
