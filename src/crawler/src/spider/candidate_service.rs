use async_trait::async_trait;
use common::shop_id::ShopId;
use sqlx::PgPool;

pub struct SpiderCandidate {
    pub shop_id: ShopId,
    pub shop_domain: String,
}

#[async_trait]
#[mockall::automock]
pub trait SpiderCandidateService: Send + Sync {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<SpiderCandidate>, sqlx::Error>;
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
    shop_domain: String,
}

#[async_trait]
impl SpiderCandidateService for SpiderCandidateServiceImpl {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<SpiderCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, SpiderCandidateRow>(
            r#"
            SELECT s.shop_id, sd.shop_domain
            FROM shops s
            JOIN shop_domains sd ON sd.shop_id = s.shop_id
            WHERE s.active = TRUE
              AND (sd.last_crawled IS NULL OR sd.last_crawled < NOW() - INTERVAL '7 days')
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
                shop_domain: row.shop_domain,
            })
            .collect())
    }
}
