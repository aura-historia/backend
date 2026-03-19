use async_trait::async_trait;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderLinkRecord {
    pub shop_url: String,
    pub url: String,
    pub link_class: String,
    pub main_hash: String,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderLinkRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            shop_url: row.try_get("shop_url")?,
            url: row.try_get("url")?,
            link_class: row.try_get("link_class")?,
            main_hash: row.try_get("main_hash")?,
            last_scraped: row.try_get("last_scraped")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

#[async_trait]
#[mockall::automock]
pub trait LinkMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        shop_url: &str,
        url: &str,
        link_class: &str,
        main_hash: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;

    async fn mark_as_scraped(
        &self,
        shop_url: &str,
        url: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;
}

pub struct LinkMetadataRepositoryImpl {
    pool: PgPool,
}

impl LinkMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LinkMetadataRepository for LinkMetadataRepositoryImpl {
    async fn upsert_link(
        &self,
        shop_url: &str,
        url: &str,
        link_class: &str,
        main_hash: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        sqlx::query_as::<_, SpiderLinkRecord>(
            "INSERT INTO spider_link (shop_url, url, link_class, main_hash, created, updated)
             VALUES ($1, $2, $3, $4, NOW(), NOW())
             ON CONFLICT (shop_url, url)
             DO UPDATE SET
                 link_class = EXCLUDED.link_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_url, url, link_class, main_hash, last_scraped, created, updated",
        )
        .bind(shop_url)
        .bind(url)
        .bind(link_class)
        .bind(main_hash)
        .fetch_one(&self.pool)
        .await
    }

    async fn mark_as_scraped(
        &self,
        shop_url: &str,
        url: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        sqlx::query_as::<_, SpiderLinkRecord>(
            "UPDATE spider_link
             SET last_scraped = NOW(), updated = NOW()
             WHERE shop_url = $1 AND url = $2
             RETURNING shop_url, url, link_class, main_hash, last_scraped, created, updated",
        )
        .bind(shop_url)
        .bind(url)
        .fetch_one(&self.pool)
        .await
    }
}
