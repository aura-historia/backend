use async_trait::async_trait;
use common::shop_id::ShopId;
use sqlx::PgPool;

pub struct SpiderCandidate {
    pub shop_id: ShopId,
    pub domain_id: uuid::Uuid,
    pub shop_domain: String,
}

#[async_trait]
#[mockall::automock]
pub trait SpiderCandidateService: Send + Sync {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<SpiderCandidate>, sqlx::Error>;
    async fn mark_crawl_failure(
        &self,
        domain_id: &uuid::Uuid,
        error_kind: &str,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
    async fn reset_crawl_failure(&self, domain_id: &uuid::Uuid) -> Result<(), sqlx::Error>;
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
    shop_id: uuid::Uuid,
    domain_id: uuid::Uuid,
    shop_domain: String,
}

#[async_trait]
impl SpiderCandidateService for SpiderCandidateServiceImpl {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<SpiderCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, SpiderCandidateRow>(
            r#"
            SELECT s.shop_id, sd.domain_id, sd.shop_domain
            FROM shops s
            JOIN shop_domains sd ON sd.shop_id = s.shop_id
            WHERE s.active = TRUE
              AND (sd.last_crawled IS NULL OR sd.last_crawled < NOW() - INTERVAL '7 days')
              AND (sd.next_crawl_at IS NULL OR sd.next_crawl_at <= NOW())
            ORDER BY sd.last_crawled NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| SpiderCandidate {
                shop_id: ShopId::from(row.shop_id),
                domain_id: row.domain_id,
                shop_domain: row.shop_domain,
            })
            .collect())
    }

    async fn mark_crawl_failure(
        &self,
        domain_id: &uuid::Uuid,
        error_kind: &str,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE shop_domains
             SET crawl_failure_count = crawl_failure_count + 1,
                 last_crawl_error_kind = $2,
                 next_crawl_at = $3
             WHERE domain_id = $1",
        )
        .bind(domain_id)
        .bind(error_kind)
        .bind(next_crawl_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn reset_crawl_failure(&self, domain_id: &uuid::Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE shop_domains
             SET crawl_failure_count = 0,
                 last_crawl_error_kind = NULL,
                 next_crawl_at = NULL
             WHERE domain_id = $1",
        )
        .bind(domain_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
