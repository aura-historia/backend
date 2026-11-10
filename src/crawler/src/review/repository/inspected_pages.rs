//! The pages of a review's ListingSource that the console projected for the review, as an agent's
//! `inspect_page` reads them: what the projection found on a URL-pattern review's sample page, and
//! which pages of a product-schema review's ListingSource were inspected, the only pages an agent's
//! [no-sold-page record](super::no_sold_page) may name. The contract is in
//! `docs/crawler/extraction-and-review.md` (approval guard).

use super::CrawlerReviewRepository;
use super::reviewer_page_url::ReviewPageIdentity;
use super::url_pattern_sample::record_sample_page_markup;
use crate::CrawlerReviewId;
use crate::review::model::ReviewActor;
use std::collections::HashSet;
use url::Url;

impl CrawlerReviewRepository {
    /// Records that the console projected the page of `url` for the review: on a URL-pattern
    /// review's sample row of the page, whether it describes exactly one schema.org `Product`; on a
    /// product-schema review, that the page was inspected now.
    pub async fn record_inspected_page(
        &self,
        review_id: CrawlerReviewId,
        url: &Url,
        describes_one_product: bool,
    ) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        record_sample_page_markup(&mut transaction, review_id, url, describes_one_product).await?;
        sqlx::query(
            "INSERT INTO crawler_review_inspected_pages (review_id, url) \
             SELECT review_id, $2 FROM crawler_reviews \
             WHERE review_id = $1 AND artifact_type = 'PRODUCT_SCHEMA' \
             ON CONFLICT (review_id, url) DO UPDATE SET inspected = clock_timestamp()",
        )
        .bind(review_id.as_uuid())
        .bind(url.as_str())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }
}

/// The pages the console projected for the review since its latest `HUMAN` event, or ever when no
/// human acted on it: a human's change or verdict hands the review back, and what the agents saw
/// before it no longer stands for them.
pub(super) async fn pages_inspected_since_last_human_event<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
) -> Result<HashSet<ReviewPageIdentity>, sqlx::Error> {
    let urls = sqlx::query_scalar::<_, String>(
        "SELECT i.url FROM crawler_review_inspected_pages i \
         WHERE i.review_id = $1 AND i.inspected > COALESCE( \
             (SELECT MAX(h.created) FROM crawler_review_events h \
              WHERE h.review_id = $1 AND h.actor = $2), \
             '-infinity'::timestamptz)",
    )
    .bind(review_id.as_uuid())
    .bind(ReviewActor::Human.as_str())
    .fetch_all(executor)
    .await?;
    Ok(urls
        .iter()
        .filter_map(|url| ReviewPageIdentity::of_stored(url))
        .collect())
}
