use crate::CrawlerDomainId;
use crate::spider::discovery::website_spider::CrawlCompletion;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::time::Duration;

/// A domain is due again once its last full crawl is older than this.
pub const SPIDER_RECRAWL_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Persisted `last_crawl_outcome` of a run that ended in a spider error.
const FAILED_CRAWL_OUTCOME: &str = "FAILED";

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
              AND NOT (sd.domain_id = ANY($2))
            ORDER BY sd.last_crawled NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .bind(excluded_domain_ids)
        .bind(SPIDER_RECRAWL_INTERVAL.as_secs_f64())
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
        debug_assert!(completion.is_full(), "truncated crawls are not completed");
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
}
