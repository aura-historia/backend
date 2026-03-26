use async_trait::async_trait;
use common::shop_id::ShopId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderLinkRecord {
    pub shop_id: ShopId,
    pub url: String,
    pub link_class: String,
    pub main_hash: String,
    pub state: String,
    pub price_currency: Option<String>,
    pub price_value: Option<i32>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderLinkRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let shop_id: uuid::Uuid = row.try_get("shop_id")?;
        Ok(Self {
            shop_id: shop_id.into(),
            url: row.try_get("url")?,
            link_class: row.try_get("link_class")?,
            main_hash: row.try_get("main_hash")?,
            state: row.try_get("state")?,
            price_currency: row.try_get("price_currency")?,
            price_value: row.try_get("price_value")?,
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
        shop_id: &ShopId,
        url: &str,
        link_class: &str,
        main_hash: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[String],
        link_classes: &[String],
        main_hashes: &[String],
    ) -> Result<Vec<SpiderLinkRecord>, sqlx::Error>;

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &str,
        state: &str,
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
        shop_id: &ShopId,
        url: &str,
        link_class: &str,
        main_hash: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "INSERT INTO spider_link (shop_id, url, link_class, main_hash, created, updated)
             VALUES ($1, $2, $3, $4, NOW(), NOW())
             ON CONFLICT (shop_id, url)
             DO UPDATE SET
                 link_class = EXCLUDED.link_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url)
        .bind(link_class)
        .bind(main_hash)
        .fetch_one(&self.pool)
        .await
    }

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[String],
        link_classes: &[String],
        main_hashes: &[String],
    ) -> Result<Vec<SpiderLinkRecord>, sqlx::Error> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "INSERT INTO spider_link (shop_id, url, link_class, main_hash, created, updated)
             SELECT $1, t.url, t.link_class, t.main_hash, NOW(), NOW()
             FROM UNNEST($2::text[], $3::text[], $4::text[]) AS t(url, link_class, main_hash)
             ON CONFLICT (shop_id, url)
             DO UPDATE SET
                 link_class = EXCLUDED.link_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(urls)
        .bind(link_classes)
        .bind(main_hashes)
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "UPDATE spider_link
             SET last_scraped = NOW(), updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url)
        .fetch_one(&self.pool)
        .await
    }

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &str,
        state: &str,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "UPDATE spider_link
             SET state = $3, updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url)
        .bind(state)
        .fetch_one(&self.pool)
        .await
    }
}
