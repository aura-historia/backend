//! Marking a review page of a pending product-schema review as no product page, as the author does
//! for the dealer's profile or a category page the URL pattern matches. A marked page is left out
//! of the schema matrix and the review's coverage, so the candidate need not resolve it, and
//! approving the review retires its URL as a product URL of the ListingSource; the spider keeps it
//! retired through [`RejectedPages`](super::RejectedPages). The contract is in
//! `docs/crawler/extraction-and-review.md` (review pages).

use super::rejected_pages::store_url_classes_clearing_retry;
use super::review_events::{ReviewMutation, commit_with_event};
use super::reviewer_page_url::ReviewPageIdentity;
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, ReviewUnderEdit, drop_cached_schema_matrix,
    lock_review_for_change, row_to_page,
};
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, CrawlerReviewPage, ReviewActor, STATUS_PENDING_REVIEW,
};
use crate::spider::classification::url_metadata::UrlClass;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::ListingSourceId;
use sqlx::Row;
use tracing::info;

/// The longest reason a mark keeps, in characters.
pub const MAX_NOT_PRODUCT_REASON_CHARS: usize = 500;

impl CrawlerReviewRepository {
    /// Marks the page `review_page_id` of the pending product-schema review as no product page,
    /// saying in `reason` what the page shows, and drops the review's stored schema matrix. The
    /// author agent and humans mark pages; a page already marked takes the new reason.
    pub async fn mark_page_not_product(
        &self,
        review_id: CrawlerReviewId,
        review_page_id: CrawlerReviewPageId,
        actor: ReviewActor,
        reason: &str,
    ) -> Result<CrawlerReviewPage, ReviewRepositoryError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_NOT_PRODUCT_REASON_CHARS {
            return Err(ReviewRepositoryError::InvalidNotProductReason);
        }
        let mut transaction = self.pool.begin().await?;
        lock_pending_product_schema_review(
            &mut transaction,
            review_id,
            actor,
            &self.agent_decisions,
        )
        .await?;
        let page =
            set_not_product_reason(&mut transaction, review_id, review_page_id, Some(reason))
                .await?;
        drop_cached_schema_matrix(&mut transaction, review_id).await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::PageMarkedNotProduct {
                review_page_id,
                url: &page.url,
                reason,
            },
        )
        .await?;
        info!(
            review_id = %review_id,
            actor = %actor,
            review_page_id = %review_page_id,
            "Review page marked as no product page"
        );
        Ok(page)
    }

    /// Clears the mark of the page `review_page_id` of the pending product-schema review, so the
    /// candidate has to resolve it again, and drops the review's stored schema matrix.
    pub async fn clear_page_not_product(
        &self,
        review_id: CrawlerReviewId,
        review_page_id: CrawlerReviewPageId,
        actor: ReviewActor,
    ) -> Result<CrawlerReviewPage, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        lock_pending_product_schema_review(
            &mut transaction,
            review_id,
            actor,
            &self.agent_decisions,
        )
        .await?;
        let page =
            set_not_product_reason(&mut transaction, review_id, review_page_id, None).await?;
        drop_cached_schema_matrix(&mut transaction, review_id).await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::PageNotProductCleared {
                review_page_id,
                url: &page.url,
            },
        )
        .await?;
        info!(
            review_id = %review_id,
            actor = %actor,
            review_page_id = %review_page_id,
            "Mark of a review page as no product page cleared"
        );
        Ok(page)
    }
}

/// Locks the review for `actor`'s change of its pages and refuses it unless the review is a
/// pending product-schema review, with the refusals of every other change of a review.
async fn lock_pending_product_schema_review(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<(), ReviewRepositoryError> {
    let (
        ReviewUnderEdit {
            artifact_type,
            status,
            ..
        },
        _,
    ) = lock_review_for_change(transaction, review_id, actor, agent_decisions).await?;
    if artifact_type != ARTIFACT_PRODUCT_SCHEMA {
        return Err(ReviewRepositoryError::UnsupportedArtifact(
            review_id,
            artifact_type,
        ));
    }
    if status != STATUS_PENDING_REVIEW {
        return Err(ReviewRepositoryError::NotPending(review_id));
    }
    Ok(())
}

/// Sets the mark of the page of the locked review to `reason`, `None` clearing it, and returns the
/// page as it is now. Clearing a page that is not marked is refused.
async fn set_not_product_reason(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    review_page_id: CrawlerReviewPageId,
    reason: Option<&str>,
) -> Result<CrawlerReviewPage, ReviewRepositoryError> {
    let page = sqlx::query(
        "UPDATE crawler_review_pages SET not_product_reason = $3 \
         WHERE review_page_id = $1 AND review_id = $2 \
           AND ($3::text IS NOT NULL OR not_product_reason IS NOT NULL) \
         RETURNING review_page_id, review_id, url, role, html_hash, fetched, not_product_reason",
    )
    .bind(review_page_id.as_uuid())
    .bind(review_id.as_uuid())
    .bind(reason)
    .fetch_optional(&mut **transaction)
    .await?;
    match page {
        Some(page) => Ok(row_to_page(page)?),
        None => Err(missing_page_error(transaction, review_id, review_page_id).await?),
    }
}

/// Why no page was updated: the review has no such page, or the page was not marked.
async fn missing_page_error(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    review_page_id: CrawlerReviewPageId,
) -> Result<ReviewRepositoryError, sqlx::Error> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM crawler_review_pages \
         WHERE review_page_id = $1 AND review_id = $2)",
    )
    .bind(review_page_id.as_uuid())
    .bind(review_id.as_uuid())
    .fetch_one(&mut **transaction)
    .await?;
    Ok(if exists {
        ReviewRepositoryError::ReviewPageNotMarkedNotProduct(review_page_id)
    } else {
        ReviewRepositoryError::ReviewPageNotFound(review_page_id)
    })
}

/// Retires the URLs of the review's marked pages as product URLs of the ListingSource inside the
/// approval's transaction: every stored spelling of each becomes class `other` with its retry
/// delay cleared, as a rejection classifies its pages. The mark keeps the pages `other` on later
/// crawls (see [`RejectedPages`](super::RejectedPages)). Returns the marked pages' URLs, which
/// the approval's event names.
pub(super) async fn retire_marked_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
) -> Result<Vec<String>, ReviewRepositoryError> {
    let marked_urls = sqlx::query_scalar::<_, String>(
        "SELECT url FROM crawler_review_pages \
         WHERE review_id = $1 AND not_product_reason IS NOT NULL ORDER BY created, review_page_id",
    )
    .bind(review_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await?;
    if marked_urls.is_empty() {
        return Ok(marked_urls);
    }
    let spellings: Vec<String> = marked_urls
        .iter()
        .flat_map(|url| ReviewPageIdentity::stored_spellings(url))
        .collect();
    let stored = sqlx::query(
        "SELECT url FROM listing_source_urls \
         WHERE listing_source_id = $1 AND url = ANY($2::text[]) \
           AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD') AND url_class <> $3 \
         FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(&spellings)
    .bind(UrlClass::Other.as_str())
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|row| row.try_get::<String, _>("url"))
    .collect::<Result<Vec<_>, _>>()?;
    let classes = vec![UrlClass::Other.to_string(); stored.len()];
    if !stored.is_empty() {
        store_url_classes_clearing_retry(transaction, listing_source_id, &stored, &classes).await?;
    }
    info!(
        review_id = %review_id,
        marked_pages = marked_urls.len(),
        retired_urls = stored.len(),
        "Retired the URLs of the pages marked as no product page"
    );
    Ok(marked_urls)
}
