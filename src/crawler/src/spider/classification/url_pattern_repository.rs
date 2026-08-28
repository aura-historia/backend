//! Repository for persisting and retrieving product URL patterns per shop.
//!
//! A *shop URL pattern* is a regex string that identifies product pages within a given shop's
//! domain. Once a pattern has been discovered by the spider it is stored here so that subsequent
//! runs can skip the classification step and use the cached pattern directly.
//!
//! Locking is handled at the crawler dispatcher level via in-memory domain/url locks; see
//! [`crate::spider::advisory_lock`]. No `locked_at` column or lock methods live here.

use async_trait::async_trait;
use listing_source_core::Domain;
use listing_source_core::ListingSourceId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

/// A full row joined from the `listing_sources` and `listing_source_domains` tables.
#[derive(Debug, Clone)]
pub struct ListingSourceUrlPatternRecord {
    /// The unique shop identifier used as the primary key.
    pub listing_source_id: ListingSourceId,
    /// The domain of the shop.
    pub listing_source_domain: Domain,
    /// The stored regex pattern, if any has been confirmed for this shop.
    pub url_pattern: Option<String>,
    /// When the shop domain was last crawled successfully.
    pub last_crawled: Option<OffsetDateTime>,
    /// When this record was first created.
    pub created: OffsetDateTime,
    /// When this record was last updated.
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for ListingSourceUrlPatternRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let listing_source_domain_str: String = row.try_get("listing_source_domain")?;
        let listing_source_domain = Domain::try_from(listing_source_domain_str)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;

        Ok(Self {
            listing_source_id: listing_source_id.into(),
            listing_source_domain,
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
pub trait ListingSourceUrlPatternRepository: Send + Sync {
    /// Returns the stored record for `listing_source_id`, or `None` if none has been saved yet.
    async fn find_pattern(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error>;

    /// Persists `pattern` for `listing_source_id` with `listing_source_domain`.
    ///
    /// On first write `created` is set to the current time. Subsequent writes only
    /// update `pattern` and `updated`, leaving `created` untouched.
    ///
    /// Passing `None` explicitly clears the pattern for the given shop.
    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        listing_source_domain: &Domain,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    /// Marks the shop domain as having been crawled now.
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        listing_source_domain: &Domain,
    ) -> Result<(), sqlx::Error>;

    /// Increments the per-shop LLM call counter.
    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;
}

/// PostgreSQL-backed implementation of [`ListingSourceUrlPatternRepository`].
///
/// Patterns are stored in the `listing_sources` table keyed by `listing_source_id`.
/// Domain-level crawl state (`last_crawled`) lives in `listing_source_domains`.
/// Locking is delegated to the cron-level in-memory lock manager; see [`crate::spider::advisory_lock`].
pub struct ListingSourceUrlPatternRepositoryImpl {
    pool: PgPool,
}

impl ListingSourceUrlPatternRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListingSourceUrlPatternRepository for ListingSourceUrlPatternRepositoryImpl {
    async fn find_pattern(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        sqlx::query_as::<_, ListingSourceUrlPatternRecord>(
            "SELECT s.listing_source_id, sd.listing_source_domain, s.url_pattern, sd.last_crawled,
                    s.created, s.updated
             FROM listing_sources s
             JOIN listing_source_domains sd ON sd.listing_source_id = s.listing_source_id
             WHERE s.listing_source_id = $1
             LIMIT 1",
        )
        .bind(listing_source_id_uuid)
        .fetch_optional(&self.pool)
        .await
    }

    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        listing_source_domain: &Domain,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();

        // Upsert the shop row (url_pattern lives here)
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, url_pattern, created, updated)
             VALUES ($1, $2, NOW(), NOW())
             ON CONFLICT (listing_source_id)
             DO UPDATE SET
                 url_pattern = EXCLUDED.url_pattern,
                 updated = NOW()",
        )
        .bind(listing_source_id_uuid)
        .bind(pattern)
        .execute(&self.pool)
        .await?;

        // Upsert the domain row
        sqlx::query(
            "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain)
             VALUES ($1, $2)
             ON CONFLICT (listing_source_domain) DO NOTHING",
        )
        .bind(listing_source_id_uuid)
        .bind(listing_source_domain.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        listing_source_domain: &Domain,
    ) -> Result<(), sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();

        // Ensure the shop exists
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, created, updated)
             VALUES ($1, NOW(), NOW())
             ON CONFLICT (listing_source_id) DO NOTHING",
        )
        .bind(listing_source_id_uuid)
        .execute(&self.pool)
        .await?;

        // Upsert domain and stamp last_crawled
        sqlx::query(
            "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain, last_crawled)
             VALUES ($1, $2, NOW())
             ON CONFLICT (listing_source_domain)
             DO UPDATE SET last_crawled = NOW()",
        )
        .bind(listing_source_id_uuid)
        .bind(listing_source_domain.as_str())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        sqlx::query(
            "UPDATE listing_sources
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE listing_source_id = $1",
        )
        .bind(listing_source_id_uuid)
        .bind(delta)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
