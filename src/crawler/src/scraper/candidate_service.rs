//! Service for fetching scraper candidates — product URLs that are due for re-scraping.
//!
//! A scraper candidate is a URL stored in `shop_urls` that is due for scraping by recency and
//! retry/state rules. Hash comparison is performed in-memory by the scraper after fetching HTML.
//! Each candidate carries the shop metadata (`shop_id`, `shop_name`, `shop_type`) needed to build
//! an [`UpsertProductCommand`] without an additional lookup.

use async_trait::async_trait;
use common::shop_id::ShopId;
use shop::core::shop_type::ShopType;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::service::shop_registration::shop_type_from_db;
use crate::spider::classification::url_metadata::UrlState;

pub struct ScraperCandidate {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub shop_type: ShopType,
    pub url: Url,
    pub last_scraped_hash: Option<String>,
}

#[async_trait]
#[mockall::automock]
pub trait ScraperCandidateService: Send + Sync {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
    ) -> Result<(), sqlx::Error>;
    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) -> Result<(), sqlx::Error>;
    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
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
    shop_name: String,
    shop_type: Option<String>,
    url: String,
    last_scraped_hash: Option<String>,
}

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(&self, limit: i64) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(
            r#"
            SELECT su.shop_id, s.shop_name, s.shop_type, su.url, su.last_scraped_hash
            FROM shop_urls su
            JOIN shops s ON s.shop_id = su.shop_id
            WHERE s.active = TRUE
              AND su.url_class = 'product'
              AND su.state IN ('AVAILABLE', 'UNKNOWN', 'LISTED', 'RESERVED')
              AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
              AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
            ORDER BY su.last_scraped NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut candidates = Vec::new();
        for row in rows {
            let Some(url) = Url::parse(&row.url).ok() else {
                continue;
            };
            let Some(shop_type) = shop_type_from_db(row.shop_type.as_deref()) else {
                continue;
            };
            candidates.push(ScraperCandidate {
                shop_id: ShopId::from(row.shop_id),
                shop_name: row.shop_name,
                shop_type,
                url,
                last_scraped_hash: row.last_scraped_hash,
            });
        }

        Ok(candidates)
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &Url,
        hash: &str,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let state_str = state.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET state = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(state_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_fetch_failure(
        &self,
        shop_id: &ShopId,
        url: &Url,
        error_kind: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();

        sqlx::query(
            "UPDATE shop_urls
             SET failure_count = failure_count + 1,
                 last_error_kind = $3,
                 last_status_code = $4,
                 next_retry_at = $5,
                 updated = NOW()
             WHERE shop_id = $1 AND url = $2",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(status_code)
        .bind(next_retry_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
