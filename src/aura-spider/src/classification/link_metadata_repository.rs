use crate::domain::{LinkClass, LinkState};
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
pub struct SpiderLinkRecord {
    pub shop_id: ShopId,
    pub url: url::Url,
    pub link_class: LinkClass,
    pub main_hash: MainHash,
    pub state: LinkState,
    pub price_currency: Option<String>,
    pub price_value: Option<u32>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderLinkRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let shop_id: uuid::Uuid = row.try_get("shop_id")?;
        let url_str: String = row.try_get("url")?;
        let url = url::Url::parse(&url_str).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        let link_class_str: String = row.try_get("link_class")?;
        let link_class = std::str::FromStr::from_str(&link_class_str)
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
            link_class,
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
pub trait LinkMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        link_class: &LinkClass,
        main_hash: &MainHash,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[url::Url],
        link_classes: &[LinkClass],
        main_hashes: &[MainHash],
    ) -> Result<Vec<SpiderLinkRecord>, sqlx::Error>;

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
    ) -> Result<SpiderLinkRecord, sqlx::Error>;

    async fn set_state(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        state: &LinkState,
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
        url: &url::Url,
        link_class: &LinkClass,
        main_hash: &MainHash,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let link_class_str = link_class.to_string();
        let main_hash_str = main_hash.to_string();

        sqlx::query_as::<_, SpiderLinkRecord>(
            "INSERT INTO spider_link (shop_id, url, link_class, main_hash, created, updated)
             VALUES ($1, $2, $3, $4, NOW(), NOW())
             ON CONFLICT (url)
             DO UPDATE SET
                 link_class = EXCLUDED.link_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(link_class_str)
        .bind(main_hash_str)
        .fetch_one(&self.pool)
        .await
    }

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        urls: &[url::Url],
        link_classes: &[LinkClass],
        main_hashes: &[MainHash],
    ) -> Result<Vec<SpiderLinkRecord>, sqlx::Error> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_strs: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
        let link_class_strs: Vec<String> = link_classes.iter().map(|c| c.to_string()).collect();
        let main_hash_strs: Vec<String> = main_hashes.iter().map(|h| h.to_string()).collect();

        sqlx::query_as::<_, SpiderLinkRecord>(
            "INSERT INTO spider_link (shop_id, url, link_class, main_hash, created, updated)
             SELECT $1, t.url, t.link_class, t.main_hash, NOW(), NOW()
             FROM UNNEST($2::text[], $3::text[], $4::text[]) AS t(url, link_class, main_hash)
             ON CONFLICT (url)
             DO UPDATE SET
                 link_class = EXCLUDED.link_class,
                 main_hash = EXCLUDED.main_hash,
                 updated = NOW()
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_strs)
        .bind(link_class_strs)
        .bind(main_hash_strs)
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "UPDATE spider_link
             SET last_scraped = NOW(), updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
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
        state: &LinkState,
    ) -> Result<SpiderLinkRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let state_str = state.to_string();
        sqlx::query_as::<_, SpiderLinkRecord>(
            "UPDATE spider_link
             SET state = $3, updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, url, link_class, main_hash, state, price_currency, price_value, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(state_str)
        .fetch_one(&self.pool)
        .await
    }
}
