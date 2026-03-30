use crate::spider::classification::url_metadata::{UrlClass, UrlState};
use async_trait::async_trait;
use common::shop_id::ShopId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainHash(pub String);

impl std::fmt::Display for MainHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub struct SpiderUrlRecord {
    pub shop_id: ShopId,
    pub url: url::Url,
    pub url_class: UrlClass,
    pub main_hash: MainHash,
    pub state: UrlState,
    pub price_currency: Option<String>,
    pub price_value: Option<u32>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderUrlRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let shop_id: uuid::Uuid = row.try_get("shop_id")?;
        let url_str: String = row.try_get("url")?;
        let url = url::Url::parse(&url_str).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        let url_class_str: String = row.try_get("url_class")?;
        let url_class = std::str::FromStr::from_str(&url_class_str)
            .map_err(|e: String| sqlx::Error::Decode(e.into()))?;
        let main_hash_str: String = row.try_get("main_hash")?;
        let main_hash = MainHash(main_hash_str);
        let state_str: String = row.try_get("state")?;
        let state = std::str::FromStr::from_str(&state_str)
            .map_err(|e: String| sqlx::Error::Decode(e.into()))?;
        let price_value: Option<i32> = row.try_get("price_value")?;

        Ok(Self {
            shop_id: shop_id.into(),
            url,
            url_class,
            main_hash,
            state,
            price_currency: row.try_get("price_currency")?,
            price_value: price_value.map(|v| v as u32),
            last_scraped: row.try_get("last_scraped")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

#[async_trait]
#[mockall::automock]
pub trait UrlMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        url_class: &UrlClass,
        main_hash: &MainHash,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[url::Url],
        url_classes: &[UrlClass],
        main_hashes: &[MainHash],
    ) -> Result<Vec<SpiderUrlRecord>, sqlx::Error>;

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        state: &UrlState,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;
}

pub struct UrlMetadataRepositoryImpl {
    pool: PgPool,
}

impl UrlMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UrlMetadataRepository for UrlMetadataRepositoryImpl {
    async fn upsert_link(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        url_class: &UrlClass,
        main_hash: &MainHash,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let url_class_str = url_class.to_string();
        let main_hash_str = main_hash.to_string();

        sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO spider_link (shop_id, url, url_class, main_hash, created, updated)
             VALUES ($1, $2, $3, $4, NOW(), NOW())
             ON CONFLICT (url)
             DO UPDATE SET
                 url_class = EXCLUDED.url_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, url_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(url_class_str)
        .bind(main_hash_str)
        .fetch_one(&self.pool)
        .await
    }

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[url::Url],
        url_classes: &[UrlClass],
        main_hashes: &[MainHash],
    ) -> Result<Vec<SpiderUrlRecord>, sqlx::Error> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_strs: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
        let url_class_strs: Vec<String> = url_classes.iter().map(|c| c.to_string()).collect();
        let main_hash_strs: Vec<String> = main_hashes.iter().map(|h| h.to_string()).collect();

        sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO spider_link (shop_id, url, url_class, main_hash, created, updated)
             SELECT $1, t.url, t.url_class, t.main_hash, NOW(), NOW()
             FROM UNNEST($2::text[], $3::text[], $4::text[]) AS t(url, url_class, main_hash)
             ON CONFLICT (url)
             DO UPDATE SET
                 url_class = EXCLUDED.url_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, url_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_strs)
        .bind(url_class_strs)
        .bind(main_hash_strs)
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        sqlx::query_as::<_, SpiderUrlRecord>(
            "UPDATE spider_link
             SET last_scraped = NOW(), updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, url_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .fetch_one(&self.pool)
        .await
    }

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        state: &UrlState,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let state_str = state.to_string();
        sqlx::query_as::<_, SpiderUrlRecord>(
            "UPDATE spider_link
             SET state = $3, updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, url_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(state_str)
        .fetch_one(&self.pool)
        .await
    }
}
