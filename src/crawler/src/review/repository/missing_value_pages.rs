//! Pages whose last scrape recorded `MissingRequiredValue`, for a reviewer to declare a
//! removed-page template on (`docs/crawler/extraction-and-review.md`, removed-page templates).
//! Such a page opens and joins no review on its own, except in `agent` mode, where the scraper
//! opens or joins the review for it unless a rejected `missing_required_value` review already
//! judged the page under the live schemas; otherwise the reviewer opens one for it.

use super::reviewer_page_url::ReviewPageIdentity;
use super::{
    CrawlerReviewRepository, ReviewPlacement, ReviewRepositoryError, SchemaReviewWithStatusInput,
    parse_product_schema_payload, review_source_url_pattern, reviewer_page_url,
};
use crate::CrawlerReviewId;
use crate::review::model::{PAGE_ROLE_WAITING_PAGE, STATUS_PENDING_REVIEW, SchemaReviewPageInput};
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
        if is_redirect_to_non_product_page(requested_url, &url, product_url_pattern.as_ref()) {
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
        self.open_missing_value_review(listing_source_id, &live, Vec::new())
            .await
            .map(ReviewPlacement::review_id)
    }

    /// Opens, or joins, the ListingSource's pending product-schema review for a page the
    /// scraper just found with a required value blank, with the page it already fetched as a
    /// waiting page: no extra fetch, once per page, and at most [`MAX_WAITING_PAGES`] waiting
    /// pages however many pages of the source leave the value blank. The review is the one the
    /// console opens for the page, with the same reason and candidate. Called in `agent` mode
    /// only, where no human adds the page. `None` when a rejection already judged the page, see
    /// [`Self::rejected_missing_value_review_judged`]: the agents would pay for the same verdict
    /// on every scrape. The placement says whether the page opened the review or joined one.
    pub async fn open_or_join_review_for_scraped_missing_value_page(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_html: &str,
    ) -> Result<Option<ReviewPlacement>, ReviewRepositoryError> {
        if self
            .rejected_missing_value_review_judged(listing_source_id, url)
            .await?
        {
            return Ok(None);
        }
        let live = self.live_product_schemas(listing_source_id).await?;
        let page = SchemaReviewPageInput {
            url: url.to_string(),
            role: PAGE_ROLE_WAITING_PAGE.to_owned(),
            raw_html: raw_html.to_owned(),
        };
        self.open_missing_value_review(listing_source_id, &live, vec![page])
            .await
            .map(Some)
    }

    /// Whether a rejected `missing_required_value` review of the ListingSource holds the page of
    /// `url`, in any spelling the crawler stores it under, and no product schema list went live
    /// after that rejection: the rejection judged the page under the schemas it still fails, so
    /// only a newly live list makes another review worthwhile. A human still opens one from the
    /// console.
    async fn rejected_missing_value_review_judged(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews r \
             JOIN crawler_review_pages p ON p.review_id = r.review_id \
             LEFT JOIN listing_source_product_schemas live \
                    ON live.listing_source_id = r.listing_source_id \
             WHERE r.listing_source_id = $1 AND r.artifact_type = 'PRODUCT_SCHEMA' \
               AND r.status = 'REJECTED' AND r.reason = $2 AND p.url = ANY($3) \
               AND r.reviewed >= COALESCE(live.updated, '-infinity'::timestamptz))",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(MISSING_REQUIRED_VALUE_REVIEW_REASON)
        .bind(ReviewPageIdentity::spellings(url))
        .fetch_one(&self.pool)
        .await
    }

    async fn open_missing_value_review(
        &self,
        listing_source_id: &ListingSourceId,
        live: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
    ) -> Result<ReviewPlacement, ReviewRepositoryError> {
        self.insert_schema_review(SchemaReviewWithStatusInput {
            listing_source_id,
            reason: MISSING_REQUIRED_VALUE_REVIEW_REASON,
            schemas: live,
            pages,
            validation_summary: serde_json::json!({}),
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
    }
}
