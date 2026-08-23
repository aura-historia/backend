use crate::spider::classification::url_metadata::{UrlClass, UrlState};
use async_trait::async_trait;
use shop_core::shop_id::ShopId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderUrlRecord {
    pub shop_id: ShopId,
    pub domain_id: uuid::Uuid,
    pub url: url::Url,
    pub url_class: UrlClass,
    pub state: UrlState,
    pub last_scraped_hash: Option<String>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderUrlRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let shop_id: uuid::Uuid = row.try_get("shop_id")?;
        let domain_id: uuid::Uuid = row.try_get("domain_id")?;
        let url_str: String = row.try_get("url")?;
        let url = url::Url::parse(&url_str).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        let url_class_str: String = row.try_get("url_class")?;
        let url_class = std::str::FromStr::from_str(&url_class_str)
            .map_err(|e: String| sqlx::Error::Decode(e.into()))?;
        let state_str: String = row.try_get("last_scraped_state")?;
        let state = std::str::FromStr::from_str(&state_str)
            .map_err(|e: String| sqlx::Error::Decode(e.into()))?;

        Ok(Self {
            shop_id: shop_id.into(),
            domain_id,
            url,
            url_class,
            state,
            last_scraped_hash: row.try_get("last_scraped_hash")?,
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
        domain_id: &uuid::Uuid,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        domain_id: &uuid::Uuid,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, sqlx::Error>;

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        hash: &str,
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
        domain_id: &uuid::Uuid,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        let url_class_str = url_class.to_string();

        sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO shop_urls (shop_id, domain_id, url, url_class, created, updated)
             VALUES ($1, $2, $3, $4, NOW(), NOW())
             ON CONFLICT (url)
             DO UPDATE SET
                 url_class = EXCLUDED.url_class,
                 domain_id = EXCLUDED.domain_id,
                 updated = NOW()
             RETURNING shop_id, domain_id, url, url_class, last_scraped_state, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(domain_id)
        .bind(url_str)
        .bind(url_class_str)
        .fetch_one(&self.pool)
        .await
    }

    async fn upsert_links_batch(
        &self,
        shop_id: &ShopId,
        domain_id: &uuid::Uuid,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, sqlx::Error> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }

        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_strs: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
        let url_class_strs: Vec<String> = url_classes.iter().map(|c| c.to_string()).collect();

        sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO shop_urls (shop_id, domain_id, url, url_class, created, updated)
             SELECT $1, $2, t.url, t.url_class, NOW(), NOW()
             FROM UNNEST($3::text[], $4::text[]) AS t(url, url_class)
             ON CONFLICT (url)
             DO UPDATE SET
                 url_class = EXCLUDED.url_class,
                 domain_id = EXCLUDED.domain_id,
                 updated = NOW()
             RETURNING shop_id, domain_id, url, url_class, last_scraped_state, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(domain_id)
        .bind(url_strs)
        .bind(url_class_strs)
        .fetch_all(&self.pool)
        .await
    }

    async fn mark_as_scraped(
        &self,
        shop_id: &ShopId,
        url: &url::Url,
        hash: &str,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = (*shop_id).into();
        let url_str = url.to_string();
        sqlx::query_as::<_, SpiderUrlRecord>(
            "UPDATE shop_urls
             SET last_scraped = NOW(), last_scraped_hash = $3, updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, domain_id, url, url_class, last_scraped_state, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(hash)
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
            "UPDATE shop_urls
             SET last_scraped_state = $3, updated = NOW()
             WHERE shop_id = $1 AND url = $2
             RETURNING shop_id, domain_id, url, url_class, last_scraped_state, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(shop_id_uuid)
        .bind(url_str)
        .bind(state_str)
        .fetch_one(&self.pool)
        .await
    }
}
