use crate::CrawlerReviewId;
use crate::crawl_root::CrawlRoot;
use crate::logging::SourceLogContext;
use crate::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use crate::spider::discovery::frontier::CrawlKind;
use crate::spider::discovery::website_spider::CrawlCompletion;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::time::Duration;

/// A ListingSource is due for a full crawl once its last full crawl is older than this.
pub const SPIDER_RECRAWL_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A ListingSource whose last full crawl is recent is due for an incremental crawl once its last
/// spider run of any kind is older than this.
pub const SPIDER_INCREMENTAL_RECRAWL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Persisted `last_crawl_outcome` of a run that ended in a spider error.
pub const FAILED_CRAWL_OUTCOME: &str = "FAILED";

/// Persisted `last_crawl_outcome` of a crawl a shop's rate-limit answer ended
/// (`CrawlCompletion::RateLimited`).
pub const RATE_LIMITED_CRAWL_OUTCOME: &str = "RATE_LIMITED";

/// Persisted `last_crawl_outcome` of a run that stopped classifying at a
/// pending URL-pattern review. The review's decision releases the ListingSource and
/// clears this outcome.
pub const AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME: &str = "AWAITING_URL_PATTERN_REVIEW";

/// Persisted `last_crawl_outcome` of a ListingSource whose crawl failures
/// kept repeating. It is not selected again until an operator resumes its crawl or changes its
/// presentation URL, which moves it to another crawl root and clears its crawl state.
pub const NEEDS_OPERATOR_CRAWL_OUTCOME: &str = "NEEDS_OPERATOR";

/// Whether [`SpiderCandidateService::mark_crawl_awaiting_url_pattern_review`] recorded the wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlPatternReviewWaitWrite {
    /// The review was still pending: the ListingSource waits for its decision.
    Recorded,
    /// The review is no longer a pending URL-pattern review of the ListingSource, typically because
    /// it was decided while the crawl ran. Nothing was written: the decision released the
    /// ListingSource, and its crawl state stays as the decision left it.
    ReviewNotPending,
    /// The ListingSource moved to another crawl root after the run started, or was removed; nothing
    /// was written ([`CrawlOutcomeWrite::RootMoved`]).
    RootMoved,
}

/// What a run whose crawl a shop's rate-limit answer stopped records,
/// whatever its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrawlRateLimitWrite {
    /// The ListingSource's new consecutive rate-limit streak.
    pub streak: i32,
    /// The earliest the ListingSource's next crawl may start.
    pub not_before: time::OffsetDateTime,
}

pub struct SpiderCandidate {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub crawl_root: CrawlRoot,
    pub crawl_failure_count: i32,
    pub last_crawl_error_kind: Option<String>,
    /// Consecutive hard-timeout truncations since the last full crawl.
    pub crawl_truncation_count: i32,
    /// The ListingSource's rate-limit streak: it grows with each crawl a shop's
    /// rate-limit answer stopped, shrinks with each crawl the wall-clock budget
    /// cut short without one and clears with a full crawl. It slows the
    /// ListingSource's next crawls.
    pub crawl_rate_limit_streak: i32,
    /// Full when the last full crawl is due, incremental otherwise.
    pub kind: CrawlKind,
}

impl SpiderCandidate {
    /// The crawl root the candidate's run crawls, which every write of the run's outcome names.
    pub fn crawled_root(&self) -> CrawledRoot {
        CrawledRoot::new(self.listing_source_id, self.crawl_root.url().as_str())
    }

    /// The source fields every crawl event of the candidate's run logs.
    pub fn source_log_context(&self) -> SourceLogContext {
        SourceLogContext {
            listing_source_id: self.listing_source_id,
            listing_source_name: self.listing_source_name.clone(),
            domain: self.crawl_root.host().to_owned(),
        }
    }
}

/// The spider scheduler's crawl candidates and the writes of their runs' outcomes. Each write names
/// the [`CrawledRoot`] of the run and records only while the ListingSource still has that crawl
/// root, so a run of a root the ListingSource moved away from leaves the move's crawl state alone.
#[async_trait]
#[mockall::automock]
pub trait SpiderCandidateService: Send + Sync {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_listing_source_ids: &[ListingSourceId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error>;
    async fn mark_crawl_failure(
        &self,
        root: &CrawledRoot,
        error_kind: &str,
        crawl_failure_count: i32,
        rate_limit: Option<CrawlRateLimitWrite>,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a full crawl (`completion.is_full()`) and clears failure,
    /// truncation, rate-limit and cooldown state.
    async fn mark_crawl_completed(
        &self,
        root: &CrawledRoot,
        completion: CrawlCompletion,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a hard-timeout truncation without touching failure state; it
    /// steps the rate-limit streak down by one.
    async fn mark_crawl_truncated(
        &self,
        root: &CrawledRoot,
        crawl_truncation_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a crawl the crawler's shutdown ended ([`CrawlCompletion::Stopped`]) without
    /// touching failure, truncation or rate-limit state; the next crawl is due at `next_crawl_at`
    /// and resumes from the frontier the crawl kept.
    async fn mark_crawl_stopped(
        &self,
        root: &CrawledRoot,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a crawl a shop's rate-limit answer ended, without touching
    /// failure or truncation state; the next crawl is due at `not_before`.
    async fn mark_crawl_rate_limited(
        &self,
        root: &CrawledRoot,
        rate_limit: CrawlRateLimitWrite,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a crawl that waits for the URL-pattern review `review_id` without touching failure
    /// or truncation state, while that review is still a pending review of the ListingSource.
    /// `next_crawl_at` is the latest the ListingSource waits; the review's decision releases it
    /// sooner.
    async fn mark_crawl_awaiting_url_pattern_review(
        &self,
        root: &CrawledRoot,
        review_id: &CrawlerReviewId,
        rate_limit: Option<CrawlRateLimitWrite>,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<UrlPatternReviewWaitWrite, sqlx::Error>;
    /// Records the end of an incremental crawl that was neither rate-limited
    /// nor waiting for a review. It never records a full crawl: a crawl that
    /// reached everything clears failure state, one cut short leaves it, and
    /// neither touches truncation or rate-limit state.
    async fn mark_incremental_crawl_finished(
        &self,
        root: &CrawledRoot,
        completion: CrawlCompletion,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
    /// Records a failure that reached its escalation count and takes the ListingSource out of
    /// the schedule until an operator acts.
    async fn mark_crawl_needs_operator(
        &self,
        root: &CrawledRoot,
        error_kind: &str,
        crawl_failure_count: i32,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error>;
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
    listing_source_name: String,
    crawl_root_url: String,
    crawl_host: String,
    crawl_failure_count: i32,
    last_crawl_error_kind: Option<String>,
    crawl_truncation_count: i32,
    crawl_rate_limit_streak: i32,
    full_crawl_due: bool,
}

#[async_trait]
impl SpiderCandidateService for SpiderCandidateServiceImpl {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_listing_source_ids: &[ListingSourceId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error> {
        let excluded_listing_source_ids: Vec<uuid::Uuid> = excluded_listing_source_ids
            .iter()
            .copied()
            .map(uuid::Uuid::from)
            .collect();
        let rows = sqlx::query_as::<_, SpiderCandidateRow>(
            r#"
            SELECT listing_source_id,
                   listing_source_name,
                   crawl_root_url,
                   crawl_host,
                   crawl_failure_count,
                   last_crawl_error_kind,
                   crawl_truncation_count,
                   crawl_rate_limit_streak,
                   (last_crawled IS NULL
                    OR last_crawled < NOW() - make_interval(secs => $3)) AS full_crawl_due
            FROM listing_sources
            WHERE crawl_enabled = TRUE
              AND crawl_root_url IS NOT NULL
              AND (last_crawled IS NULL
                   OR last_crawled < NOW() - make_interval(secs => $3)
                   OR last_crawl_run_at IS NULL
                   OR last_crawl_run_at < NOW() - make_interval(secs => $5))
              AND (next_crawl_at IS NULL OR next_crawl_at <= NOW())
              AND (crawl_rate_limited_until IS NULL OR crawl_rate_limited_until <= NOW())
              AND last_crawl_outcome IS DISTINCT FROM $4
              AND NOT (listing_source_id = ANY($2))
            ORDER BY last_crawled NULLS FIRST, last_crawl_run_at NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .bind(excluded_listing_source_ids)
        .bind(SPIDER_RECRAWL_INTERVAL.as_secs_f64())
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .bind(SPIDER_INCREMENTAL_RECRAWL_INTERVAL.as_secs_f64())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(SpiderCandidate {
                    listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    listing_source_name: row.listing_source_name,
                    crawl_root: CrawlRoot::from_persisted(&row.crawl_root_url, &row.crawl_host)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    crawl_failure_count: row.crawl_failure_count,
                    last_crawl_error_kind: row.last_crawl_error_kind,
                    crawl_truncation_count: row.crawl_truncation_count,
                    crawl_rate_limit_streak: row.crawl_rate_limit_streak,
                    kind: match row.full_crawl_due {
                        true => CrawlKind::Full,
                        false => CrawlKind::Incremental,
                    },
                })
            })
            .collect()
    }

    async fn mark_crawl_failure(
        &self,
        root: &CrawledRoot,
        error_kind: &str,
        crawl_failure_count: i32,
        rate_limit: Option<CrawlRateLimitWrite>,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_failure_count = $3,
                 last_crawl_error_kind = $2,
                 next_crawl_at = $4,
                 last_crawl_outcome = $5,
                 crawl_rate_limit_streak = COALESCE($6, crawl_rate_limit_streak),
                 crawl_rate_limited_until = COALESCE($7, crawl_rate_limited_until),
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $8",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(error_kind)
        .bind(crawl_failure_count)
        .bind(next_crawl_at)
        .bind(FAILED_CRAWL_OUTCOME)
        .bind(rate_limit.map(|rate_limit| rate_limit.streak))
        .bind(rate_limit.map(|rate_limit| rate_limit.not_before))
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    async fn mark_crawl_completed(
        &self,
        root: &CrawledRoot,
        completion: CrawlCompletion,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        // Clearing failure state for a truncated crawl is exactly the #1912 bug.
        if !completion.is_full() {
            return Err(sqlx::Error::InvalidArgument(format!(
                "{completion} is not a completed crawl outcome"
            )));
        }
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_failure_count = 0,
                 last_crawl_error_kind = NULL,
                 next_crawl_at = NULL,
                 crawl_truncation_count = 0,
                 crawl_rate_limit_streak = 0,
                 crawl_rate_limited_until = NULL,
                 last_crawl_outcome = $2,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $3",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(completion.as_str())
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    async fn mark_crawl_truncated(
        &self,
        root: &CrawledRoot,
        crawl_truncation_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_truncation_count = $2,
                 crawl_rate_limit_streak = GREATEST(crawl_rate_limit_streak - 1, 0),
                 next_crawl_at = $3,
                 last_crawl_outcome = $4,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $5",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(crawl_truncation_count)
        .bind(next_crawl_at)
        .bind(CrawlCompletion::HardTimeoutTruncated.as_str())
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    async fn mark_crawl_stopped(
        &self,
        root: &CrawledRoot,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET next_crawl_at = $2,
                 last_crawl_outcome = $3,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $4",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(next_crawl_at)
        .bind(CrawlCompletion::Stopped.as_str())
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    async fn mark_crawl_rate_limited(
        &self,
        root: &CrawledRoot,
        rate_limit: CrawlRateLimitWrite,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_rate_limit_streak = $2,
                 crawl_rate_limited_until = $3,
                 next_crawl_at = $3,
                 last_crawl_outcome = $4,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $5",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(rate_limit.streak)
        .bind(rate_limit.not_before)
        .bind(RATE_LIMITED_CRAWL_OUTCOME)
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    /// The pending check share-locks the review row before the ListingSource row is written.
    /// Approval and rejection lock the review row first and release the ListingSource before they
    /// commit, so a wait written while either verdict commits waits for it and then finds the
    /// review closed, instead of passing a check that read the review before the verdict committed
    /// and parking the ListingSource the verdict just released. A wait that wrote nothing is
    /// reported as a moved root when the ListingSource no longer has the crawled root, whatever the
    /// review's state.
    async fn mark_crawl_awaiting_url_pattern_review(
        &self,
        root: &CrawledRoot,
        review_id: &CrawlerReviewId,
        rate_limit: Option<CrawlRateLimitWrite>,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<UrlPatternReviewWaitWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET next_crawl_at = $2,
                 last_crawl_outcome = $3,
                 crawl_rate_limit_streak = COALESCE($5, crawl_rate_limit_streak),
                 crawl_rate_limited_until = COALESCE($6, crawl_rate_limited_until),
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1
               AND crawl_root_url = $7
               AND EXISTS (
                   SELECT 1
                   FROM crawler_reviews cr
                   WHERE cr.review_id = $4
                     AND cr.listing_source_id = $1
                     AND cr.artifact_type = 'URL_PATTERN'
                     AND cr.status = 'PENDING_REVIEW'
                   FOR SHARE
               )",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(next_crawl_at)
        .bind(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
        .bind(review_id.as_uuid())
        .bind(rate_limit.map(|rate_limit| rate_limit.streak))
        .bind(rate_limit.map(|rate_limit| rate_limit.not_before))
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() > 0 {
            return Ok(UrlPatternReviewWaitWrite::Recorded);
        }

        let source_has_root = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                 SELECT 1 FROM listing_sources
                 WHERE listing_source_id = $1 AND crawl_root_url = $2
             )",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(&root.crawl_root_url)
        .fetch_one(&self.pool)
        .await?;
        Ok(match source_has_root {
            true => UrlPatternReviewWaitWrite::ReviewNotPending,
            false => UrlPatternReviewWaitWrite::RootMoved,
        })
    }

    async fn mark_incremental_crawl_finished(
        &self,
        root: &CrawledRoot,
        completion: CrawlCompletion,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        if completion.rate_limit().is_some() {
            return Err(sqlx::Error::InvalidArgument(format!(
                "{completion} is recorded as a rate-limited crawl"
            )));
        }
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_failure_count = CASE WHEN $3 THEN 0 ELSE crawl_failure_count END,
                 last_crawl_error_kind = CASE WHEN $3 THEN NULL ELSE last_crawl_error_kind END,
                 next_crawl_at = NULL,
                 last_crawl_outcome = $2,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $4",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(completion.as_str())
        .bind(completion.is_full())
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }

    async fn mark_crawl_needs_operator(
        &self,
        root: &CrawledRoot,
        error_kind: &str,
        crawl_failure_count: i32,
    ) -> Result<CrawlOutcomeWrite, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET crawl_failure_count = $3,
                 last_crawl_error_kind = $2,
                 next_crawl_at = NULL,
                 last_crawl_outcome = $4,
                 last_crawl_run_at = NOW()
             WHERE listing_source_id = $1 AND crawl_root_url = $5",
        )
        .bind(root.listing_source_id.as_uuid())
        .bind(error_kind)
        .bind(crawl_failure_count)
        .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
        .bind(&root.crawl_root_url)
        .execute(&self.pool)
        .await?;

        Ok(CrawlOutcomeWrite::of_guarded_update(result.rows_affected()))
    }
}
