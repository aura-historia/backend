use crate::spider::discovery::website_spider::CrawlCompletion;
use crate::{CrawlerDomainId, CrawlerReviewId};
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::time::Duration;

/// A domain is due again once its last full crawl is older than this.
pub const SPIDER_RECRAWL_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Persisted `last_crawl_outcome` of a run that ended in a spider error.
pub const FAILED_CRAWL_OUTCOME: &str = "FAILED";

/// Persisted `last_crawl_outcome` of a run that stopped classifying at a
/// pending URL-pattern review. The review's decision releases the domain and
/// clears this outcome.
pub const AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME: &str = "AWAITING_URL_PATTERN_REVIEW";

/// Persisted `last_crawl_outcome` of a domain whose deterministic failures
/// kept repeating. It is not selected again until an operator changes its
/// configuration, which replaces the domain row.
pub const NEEDS_OPERATOR_CRAWL_OUTCOME: &str = "NEEDS_OPERATOR";

/// Whether [`SpiderCandidateService::mark_crawl_awaiting_url_pattern_review`] recorded the wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlPatternReviewWaitWrite {
    /// The review was still pending: the domain waits for its decision.
    Recorded,
    /// The review is no longer a pending URL-pattern review of the domain, typically because it
    /// was decided while the crawl ran. Nothing was written: the decision released the domain,
    /// and its crawl state stays as the decision left it.
    ReviewNotPending,
}

pub struct SpiderCandidate {
    pub listing_source_id: ListingSourceId,
    pub domain_id: CrawlerDomainId,
    pub listing_source_domain: String,
    pub crawl_failure_count: i32,
    pub last_crawl_error_kind: Option<String>,
    /// Consecutive hard-timeout truncations since the last full crawl.
    pub crawl_truncation_count: i32,
}

#[async_trait]
#[mockall::automock]
pub trait SpiderCandidateService: Send + Sync {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error>;
    async fn mark_crawl_failure(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
    /// Records a full crawl (`completion.is_full()`) and clears failure,
    /// truncation and cooldown state.
    async fn mark_crawl_completed(
        &self,
        domain_id: &CrawlerDomainId,
        completion: CrawlCompletion,
    ) -> Result<(), sqlx::Error>;
    /// Records a hard-timeout truncation without touching failure state.
    async fn mark_crawl_truncated(
        &self,
        domain_id: &CrawlerDomainId,
        crawl_truncation_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
    /// Records a crawl that waits for the URL-pattern review `review_id` without touching
    /// failure or truncation state, while that review is still a pending review of the domain.
    /// `next_crawl_at` is the latest the domain waits; the review's decision releases it sooner.
    async fn mark_crawl_awaiting_url_pattern_review(
        &self,
        domain_id: &CrawlerDomainId,
        review_id: &CrawlerReviewId,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<UrlPatternReviewWaitWrite, sqlx::Error>;
    /// Records a repeated deterministic failure and takes the domain out of
    /// the schedule until an operator acts.
    async fn mark_crawl_needs_operator(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
    ) -> Result<(), sqlx::Error>;
}

pub struct SpiderCandidateServiceImpl {
    pool: PgPool,
}

impl SpiderCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct SpiderCandidateRow {
    listing_source_id: uuid::Uuid,
    domain_id: uuid::Uuid,
    listing_source_domain: String,
    crawl_failure_count: i32,
    last_crawl_error_kind: Option<String>,
    crawl_truncation_count: i32,
}

#[async_trait]
impl SpiderCandidateService for SpiderCandidateServiceImpl {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error> {
        let excluded_domain_ids: Vec<uuid::Uuid> = excluded_domain_ids
            .iter()
            .copied()
            .map(CrawlerDomainId::into_uuid)
            .collect();
        let rows = sqlx::query_as::<_, SpiderCandidateRow>(
            r#"
            SELECT s.listing_source_id,
                   sd.domain_id,
                   sd.crawl_root_host AS listing_source_domain,
                   sd.crawl_failure_count,
                   sd.last_crawl_error_kind,
                   sd.crawl_truncation_count
            FROM listing_sources s
            JOIN listing_source_domains sd ON sd.listing_source_id = s.listing_source_id
            WHERE s.crawl_enabled = TRUE
              AND (sd.last_crawled IS NULL OR sd.last_crawled < NOW() - make_interval(secs => $3))
              AND (sd.next_crawl_at IS NULL OR sd.next_crawl_at <= NOW())
              AND sd.last_crawl_outcome IS DISTINCT FROM $4
              AND NOT (sd.domain_id = ANY($2))
            ORDER BY sd.last_crawled NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .bind(excluded_domain_ids)
        .bind(SPIDER_RECRAWL_INTERVAL.as_secs_f64())
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(SpiderCandidate {
                    listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    domain_id: CrawlerDomainId::try_from(row.domain_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    listing_source_domain: row.listing_source_domain,
                    crawl_failure_count: row.crawl_failure_count,
                    last_crawl_error_kind: row.last_crawl_error_kind,
                    crawl_truncation_count: row.crawl_truncation_count,
                })
            })
            .collect()
    }

    async fn mark_crawl_failure(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_failure_count = $3,
                 last_crawl_error_kind = $2,
                 next_crawl_at = $4,
                 last_crawl_outcome = $5
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .bind(error_kind)
        .bind(crawl_failure_count)
        .bind(next_crawl_at)
        .bind(FAILED_CRAWL_OUTCOME)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_crawl_completed(
        &self,
        domain_id: &CrawlerDomainId,
        completion: CrawlCompletion,
    ) -> Result<(), sqlx::Error> {
        // Clearing failure state for a truncated crawl is exactly the #1912 bug.
        if !completion.is_full() {
            return Err(sqlx::Error::InvalidArgument(format!(
                "{completion} is not a completed crawl outcome"
            )));
        }
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_failure_count = 0,
                 last_crawl_error_kind = NULL,
                 next_crawl_at = NULL,
                 crawl_truncation_count = 0,
                 last_crawl_outcome = $2
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .bind(completion.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_crawl_truncated(
        &self,
        domain_id: &CrawlerDomainId,
        crawl_truncation_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_truncation_count = $2,
                 next_crawl_at = $3,
                 last_crawl_outcome = $4
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .bind(crawl_truncation_count)
        .bind(next_crawl_at)
        .bind(CrawlCompletion::HardTimeoutTruncated.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// The pending check share-locks the review row before the domain row is written. Approval
    /// and rejection lock the review row first and release the domain before they commit, so a
    /// wait written while either verdict commits waits for it and then finds the review closed,
    /// instead of passing a check that read the review before the verdict committed and parking
    /// the domain the verdict just released.
    async fn mark_crawl_awaiting_url_pattern_review(
        &self,
        domain_id: &CrawlerDomainId,
        review_id: &CrawlerReviewId,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<UrlPatternReviewWaitWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains
             SET next_crawl_at = $2,
                 last_crawl_outcome = $3
             WHERE domain_id = $1
               AND EXISTS (
                   SELECT 1
                   FROM crawler_reviews cr
                   WHERE cr.review_id = $4
                     AND cr.domain_id = $1
                     AND cr.artifact_type = 'URL_PATTERN'
                     AND cr.status = 'PENDING_REVIEW'
                   FOR SHARE
               )",
        )
        .bind(domain_id.as_uuid())
        .bind(next_crawl_at)
        .bind(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
        .bind(review_id.as_uuid())
        .execute(&self.pool)
        .await?;

        Ok(match result.rows_affected() {
            0 => UrlPatternReviewWaitWrite::ReviewNotPending,
            _ => UrlPatternReviewWaitWrite::Recorded,
        })
    }

    async fn mark_crawl_needs_operator(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_failure_count = $3,
                 last_crawl_error_kind = $2,
                 next_crawl_at = NULL,
                 last_crawl_outcome = $4
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .bind(error_kind)
        .bind(crawl_failure_count)
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
