//! An operator's resumption of a ListingSource whose crawl waits for an operator after repeated
//! failures (`docs/crawler/scheduling-and-retries.md`, spider failure escalation).

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::model::ReviewActor;
use crate::spider::candidate_service::NEEDS_OPERATOR_CRAWL_OUTCOME;
use listing_source_core::ListingSourceId;
use serde::Serialize;
use sqlx::Row;

/// What `POST /api/listing-sources/{id}/crawl-resumption` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CrawlResumption {
    /// Whether the ListingSource waited for an operator; resuming one that did not changes nothing.
    pub resumed: bool,
}

impl CrawlerReviewRepository {
    /// Returns a ListingSource that waits for an operator to the spider's schedule with a new
    /// failure streak: its next crawl is due as the schedule's intervals make it, and a failure
    /// that repeats escalates again only after the full streak.
    pub async fn resume_crawl(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<CrawlResumption, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let waiting = sqlx::query(
            "SELECT last_crawl_outcome IS NOT DISTINCT FROM $2 AS waiting, \
                    last_crawl_error_kind, crawl_failure_count \
             FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::ListingSourceNotFound(
            *listing_source_id,
        ))?;
        if !waiting.try_get::<bool, _>("waiting")? {
            return Ok(CrawlResumption { resumed: false });
        }
        let error_kind: Option<String> = waiting.try_get("last_crawl_error_kind")?;
        let failure_count: i32 = waiting.try_get("crawl_failure_count")?;
        sqlx::query(
            "UPDATE listing_sources \
             SET last_crawl_outcome = NULL, last_crawl_error_kind = NULL, \
                 crawl_failure_count = 0, next_crawl_at = NULL, updated = NOW() \
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::crawl_resumed(actor, error_kind, failure_count),
        )
        .await;
        Ok(CrawlResumption { resumed: true })
    }
}
