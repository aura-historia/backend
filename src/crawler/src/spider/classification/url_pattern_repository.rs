//! Repository for persisting and retrieving product URL patterns per shop.
//!
//! A *shop URL pattern* is a regex string that identifies product pages within a given shop's
//! domain. Once a pattern has been discovered by the spider it is stored here so that subsequent
//! runs can skip the classification step and use the cached pattern directly.
//!
//! Locking is handled at the crawler dispatcher level via in-memory domain/url locks; see
//! [`crate::spider::advisory_lock`]. No `locked_at` column or lock methods live here.

use async_trait::async_trait;
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

/// A full row joined from the `shops` and `shop_domains` tables.
#[derive(Debug, Clone)]
pub struct ShopUrlPatternRecord {
    /// The unique shop identifier used as the primary key.
    pub shop_id: ShopId,
    /// The domain of the shop.
    pub shop_domain: Domain,
    /// The stored regex pattern, if any has been confirmed for this shop.
    pub url_pattern: Option<String>,
    /// When the shop domain was last crawled successfully.
    pub last_crawled: Option<OffsetDateTime>,
    /// When this record was first created.
    pub created: OffsetDateTime,
    /// When this record was last updated.
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for ShopUrlPatternRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let shop_id: uuid::Uuid = row.try_get("shop_id")?;
        let shop_domain_str: String = row.try_get("shop_domain")?;
        let shop_domain =
            Domain::try_from(shop_domain_str).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        Ok(Self {
            shop_id: shop_id.into(),
            shop_domain,
            url_pattern: row.try_get("url_pattern")?,
            last_crawled: row.try_get("last_crawled")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

/// Persistence contract for shop URL patterns.
///
/// Each shop (identified by its ID) can have at most one associated pattern.
/// The pattern is stored as a raw regex string and is `None` when no pattern has been
/// confirmed for that shop yet.
#[async_trait]
#[mockall::automock]
pub trait ShopUrlPatternRepository: Send + Sync {
    /// Returns the stored record for `shop_id`, or `None` if none has been saved yet.
    async fn find_pattern(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopUrlPatternRecord>, sqlx::Error>;

    /// Persists `pattern` for `shop_id` with `shop_domain`.
    ///
    /// On first write `created` is set to the current time. Subsequent writes only
    /// update `pattern` and `updated`, leaving `created` untouched.
    ///
    /// Passing `None` explicitly clears the pattern for the given shop.
    async fn save_pattern(
        &self,
        shop_id: &ShopId,
        shop_domain: &Domain,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// Marks the shop domain as having been crawled now.
    async fn mark_as_crawled(
        &self,
        shop_id: &ShopId,
        shop_domain: &Domain,
    ) -> Result<(), sqlx::Error>;

    /// Increments the per-shop LLM call counter.
    async fn increment_shop_llm_calls(
        &self,
        shop_id: &ShopId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;
}

/// PostgreSQL-backed implementation of [`ShopUrlPatternRepository`].
///
/// Patterns are stored in the `shops` table keyed by `shop_id`.
/// Domain-level crawl state (`last_crawled`) lives in `shop_domains`.
/// Locking is delegated to the cron-level in-memory lock manager; see [`crate::spider::advisory_lock`].
pub struct ShopUrlPatternRepositoryImpl {
    pool: PgPool,
}

impl ShopUrlPatternRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShopUrlPatternRepository for ShopUrlPatternRepositoryImpl {
    async fn find_pattern(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopUrlPatternRecord>, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query_as::<_, ShopUrlPatternRecord>(
            "SELECT s.shop_id, sd.shop_domain, s.url_pattern, sd.last_crawled,
                    s.created, s.updated
             FROM shops s
             JOIN shop_domains sd ON sd.shop_id = s.shop_id
             WHERE s.shop_id = $1
             LIMIT 1",
        )
        .bind(shop_id_uuid)
        .fetch_optional(&self.pool)
        .await
    }

    async fn save_pattern(
        &self,
        shop_id: &ShopId,
        shop_domain: &Domain,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();

        // Upsert the shop row (url_pattern lives here)
        sqlx::query(
            "INSERT INTO shops (shop_id, url_pattern, created, updated)
             VALUES ($1, $2, NOW(), NOW())
             ON CONFLICT (shop_id)
             DO UPDATE SET
                 url_pattern = EXCLUDED.url_pattern,
                 updated = NOW()",
        )
        .bind(shop_id_uuid)
        .bind(pattern)
        .execute(&self.pool)
        .await?;

        // Upsert the domain row
        sqlx::query(
            "INSERT INTO shop_domains (shop_id, shop_domain)
             VALUES ($1, $2)
             ON CONFLICT (shop_domain) DO NOTHING",
        )
        .bind(shop_id_uuid)
        .bind(shop_domain.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_as_crawled(
        &self,
        shop_id: &ShopId,
        shop_domain: &Domain,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();

        // Ensure the shop exists
        sqlx::query(
            "INSERT INTO shops (shop_id, created, updated)
             VALUES ($1, NOW(), NOW())
             ON CONFLICT (shop_id) DO NOTHING",
        )
        .bind(shop_id_uuid)
        .execute(&self.pool)
        .await?;

        // Upsert domain and stamp last_crawled
        sqlx::query(
            "INSERT INTO shop_domains (shop_id, shop_domain, last_crawled)
             VALUES ($1, $2, NOW())
             ON CONFLICT (shop_domain)
             DO UPDATE SET last_crawled = NOW()",
        )
        .bind(shop_id_uuid)
        .bind(shop_domain.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn increment_shop_llm_calls(
        &self,
        shop_id: &ShopId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query(
            "UPDATE shops
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE shop_id = $1",
        )
        .bind(shop_id_uuid)
        .bind(delta)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
