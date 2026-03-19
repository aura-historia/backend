//! Repository for persisting and retrieving product URL patterns per shop.
//!
//! A *shop URL pattern* is a regex string that identifies product pages within a given shop's
//! domain. Once a pattern has been discovered by the spider it is stored here so that subsequent
//! runs can skip the classification step and use the cached pattern directly.

use async_trait::async_trait;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

/// A full row from the `spider_shop_pattern` table.
#[derive(Debug, Clone)]
pub struct ShopUrlPatternRecord {
    /// The normalised shop origin URL used as the primary key.
    pub shop_url: String,
    /// The stored regex pattern, if any has been confirmed for this shop.
    pub url_pattern: Option<String>,
    /// When the shop was last crawled successfully.
    pub last_crawled: Option<OffsetDateTime>,
    /// When this record was first created.
    pub created: OffsetDateTime,
    /// When this record was last updated.
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for ShopUrlPatternRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            shop_url: row.try_get("shop_url")?,
            url_pattern: row.try_get("url_pattern")?,
            last_crawled: row.try_get("last_crawled")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

/// Persistence contract for shop URL patterns.
///
/// Each shop (identified by its root URL) can have at most one associated pattern.
/// The pattern is stored as a raw regex string and is `None` when no pattern has been
/// confirmed for that shop yet.
#[async_trait]
#[mockall::automock]
pub trait ShopUrlPatternRepository: Send + Sync {
    /// Returns the stored record for `shop_url`, or `None` if none has been saved yet.
    async fn find_pattern(
        &self,
        shop_url: &str,
    ) -> Result<Option<ShopUrlPatternRecord>, sqlx::Error>;

    /// Persists `pattern` for `shop_url`.
    ///
    /// On first write `created` is set to the current time. Subsequent writes only
    /// update `pattern` and `updated`, leaving `created` untouched.
    ///
    /// Passing `None` explicitly clears the pattern for the given shop.
    async fn save_pattern(&self, shop_url: &str, pattern: Option<&str>) -> Result<(), sqlx::Error>;

    /// Marks the shop as having been crawled now.
    async fn mark_as_crawled(&self, shop_url: &str) -> Result<(), sqlx::Error>;
}

/// PostgreSQL-backed implementation of [`ShopUrlPatternRepository`].
///
/// Patterns are stored in the `spider_shop_pattern` table keyed by `shop_url`.
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
        shop_url: &str,
    ) -> Result<Option<ShopUrlPatternRecord>, sqlx::Error> {
        sqlx::query_as::<_, ShopUrlPatternRecord>(
            "SELECT shop_url, url_pattern, last_crawled, created, updated
             FROM spider_shop_pattern
             WHERE shop_url = $1",
        )
        .bind(shop_url)
        .fetch_optional(&self.pool)
        .await
    }

    async fn save_pattern(&self, shop_url: &str, pattern: Option<&str>) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO spider_shop_pattern (shop_url, url_pattern, created, updated)
             VALUES ($1, $2, NOW(), NOW())
             ON CONFLICT (shop_url)
             DO UPDATE SET
                 url_pattern = EXCLUDED.url_pattern,
                 updated = NOW()",
        )
        .bind(shop_url)
        .bind(pattern)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_as_crawled(&self, shop_url: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO spider_shop_pattern (shop_url, last_crawled, created, updated)
             VALUES ($1, NOW(), NOW(), NOW())
             ON CONFLICT (shop_url)
             DO UPDATE SET
                 last_crawled = EXCLUDED.last_crawled,
                 updated = NOW()",
        )
        .bind(shop_url)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
