use async_trait::async_trait;
use common::shop_id::ShopId;
use sqlx::PgPool;
use url::Url;

pub struct ScraperCandidate {
    pub shop_id: ShopId,
    pub url: Url,
    pub main_hash: String,
    pub last_scraped_hash: Option<String>,
}

#[async_trait]
#[mockall::automock]
pub trait ScraperCandidateService: Send + Sync {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
}

pub struct ScraperCandidateServiceImpl {
    pool: PgPool,
}

impl ScraperCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct ScraperCandidateRow {
    shop_id: uuid::Uuid,
    url: String,
    main_hash: String,
    last_scraped_hash: Option<String>,
}

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(
            r#"
            SELECT shop_id, url, main_hash, last_scraped_hash
            FROM spider_link
            WHERE url_class = 'product'
              AND state IN ('AVAILABLE', 'UNKNOWN', 'LISTED', 'RESERVED')
              AND (last_scraped IS NULL OR last_scraped < NOW() - INTERVAL '1 day')
              AND (last_scraped_hash IS NULL OR main_hash != last_scraped_hash)
            ORDER BY last_scraped NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(url) = Url::parse(&row.url) {
                candidates.push(ScraperCandidate {
                    shop_id: ShopId::from(row.shop_id),
                    url,
                    main_hash: row.main_hash,
                    last_scraped_hash: row.last_scraped_hash,
                });
            }
        }

        Ok(candidates)
    }
}
