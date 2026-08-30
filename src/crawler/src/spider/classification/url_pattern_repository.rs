//! Repository for crawler-domain URL patterns and crawl scheduling state.

use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId};
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct ListingSourceUrlPatternRecord {
    pub listing_source_id: ListingSourceId,
    pub domain_id: uuid::Uuid,
    pub listing_source_domain: Domain,
    pub url_pattern: Option<String>,
    pub last_crawled: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for ListingSourceUrlPatternRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let listing_source_domain =
            Domain::try_from(row.try_get::<String, _>("listing_source_domain")?)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        Ok(Self {
            listing_source_id: listing_source_id.into(),
            domain_id: row.try_get("domain_id")?,
            listing_source_domain,
            url_pattern: row.try_get("url_pattern")?,
            last_crawled: row.try_get("last_crawled")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

#[async_trait]
#[mockall::automock]
pub trait ListingSourceUrlPatternRepository: Send + Sync {
    async fn find_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error>;

    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
    ) -> Result<(), sqlx::Error>;

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;
}

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
        domain_id: &uuid::Uuid,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error> {
        sqlx::query_as::<_, ListingSourceUrlPatternRecord>(
            "SELECT sd.listing_source_id, sd.domain_id, sd.listing_source_domain, \
                    sd.url_pattern, sd.last_crawled, s.created, s.updated \
             FROM listing_source_domains sd \
             JOIN listing_sources s ON s.listing_source_id = sd.listing_source_id \
             WHERE sd.listing_source_id = $1 AND sd.domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains \
             SET url_pattern = $3 \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id)
        .bind(pattern)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains \
             SET last_crawled = NOW() \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources \
             SET llm_calls_count = llm_calls_count + $2, updated = NOW() \
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }
}
