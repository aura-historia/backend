use crate::spider::classification::url_metadata::{UrlClass, UrlPresence};
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderUrlRecord {
    pub listing_source_id: ListingSourceId,
    pub domain_id: uuid::Uuid,
    pub url: url::Url,
    pub url_class: UrlClass,
    pub state: UrlPresence,
    pub last_scraped_hash: Option<String>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderUrlRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let domain_id: uuid::Uuid = row.try_get("domain_id")?;
        let url = url::Url::parse(&row.try_get::<String, _>("url")?)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let url_class = row
            .try_get::<String, _>("url_class")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        let state = row
            .try_get::<String, _>("last_scraped_presence")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        Ok(Self {
            listing_source_id: listing_source_id.into(),
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

#[derive(Debug, thiserror::Error)]
pub enum UrlMetadataRepositoryError {
    #[error("crawler domain does not belong to ListingSource")]
    DomainNotOwnedByListingSource {
        listing_source_id: ListingSourceId,
        domain_id: uuid::Uuid,
    },
    #[error("URL is already owned by another ListingSource")]
    UrlOwnedByAnotherListingSource {
        url: url::Url,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("crawler URL persistence failed")]
    Database {
        #[source]
        source: sqlx::Error,
    },
}

#[async_trait]
#[mockall::automock]
pub trait UrlMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError>;

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError>;

    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        hash: &str,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;

    async fn set_presence(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        state: &UrlPresence,
    ) -> Result<SpiderUrlRecord, sqlx::Error>;
}

pub struct UrlMetadataRepositoryImpl {
    pool: PgPool,
}

impl UrlMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn verify_domain_owner(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: uuid::Uuid,
    ) -> Result<(), UrlMetadataRepositoryError> {
        let owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM listing_source_domains \
                WHERE listing_source_id = $1 AND domain_id = $2 \
             )",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(database_error)?;
        if owned {
            Ok(())
        } else {
            Err(UrlMetadataRepositoryError::DomainNotOwnedByListingSource {
                listing_source_id,
                domain_id,
            })
        }
    }

    async fn conflicting_owner(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        urls: &[String],
    ) -> Result<Option<(url::Url, ListingSourceId)>, UrlMetadataRepositoryError> {
        let row = sqlx::query(
            "SELECT url, listing_source_id FROM listing_source_urls \
             WHERE url = ANY($1::text[]) AND listing_source_id <> $2 \
             ORDER BY url LIMIT 1 FOR UPDATE",
        )
        .bind(urls)
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        row.map(|row| {
            let raw_url = row.try_get::<String, _>("url").map_err(database_error)?;
            let url = url::Url::parse(&raw_url).map_err(|error| {
                UrlMetadataRepositoryError::Database {
                    source: sqlx::Error::Decode(Box::new(error)),
                }
            })?;
            let owner = row
                .try_get::<uuid::Uuid, _>("listing_source_id")
                .map_err(database_error)?;
            Ok((url, owner.into()))
        })
        .transpose()
    }

    async fn owner_for_url(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        url: &url::Url,
    ) -> Result<Option<ListingSourceId>, UrlMetadataRepositoryError> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT listing_source_id FROM listing_source_urls WHERE url = $1",
        )
        .bind(url.as_str())
        .fetch_optional(&mut **transaction)
        .await
        .map(|owner| owner.map(Into::into))
        .map_err(database_error)
    }
}

#[async_trait]
impl UrlMetadataRepository for UrlMetadataRepositoryImpl {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        Self::verify_domain_owner(&mut transaction, *listing_source_id, *domain_id).await?;
        let urls = vec![url.to_string()];
        if let Some((conflicting_url, current_listing_source_id)) =
            Self::conflicting_owner(&mut transaction, *listing_source_id, &urls).await?
        {
            return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                url: conflicting_url,
                requested_listing_source_id: *listing_source_id,
                current_listing_source_id,
            });
        }

        let record = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, created, updated) \
             VALUES ($1, $2, $3, $4, NOW(), NOW()) \
             ON CONFLICT (url) DO UPDATE SET \
                 url_class = EXCLUDED.url_class, \
                 domain_id = EXCLUDED.domain_id, \
                 updated = NOW() \
             WHERE listing_source_urls.listing_source_id = EXCLUDED.listing_source_id \
             RETURNING listing_source_id, domain_id, url, url_class, last_scraped_presence, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id)
        .bind(url.as_str())
        .bind(url_class.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;

        let Some(record) = record else {
            let current_listing_source_id = Self::owner_for_url(&mut transaction, url)
                .await?
                .ok_or_else(|| UrlMetadataRepositoryError::Database {
                    source: sqlx::Error::RowNotFound,
                })?;
            return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                url: url.clone(),
                requested_listing_source_id: *listing_source_id,
                current_listing_source_id,
            });
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(record)
    }

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &uuid::Uuid,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }
        if urls.len() != url_classes.len() {
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Protocol("URL and URL-class batch lengths differ".to_owned()),
            });
        }

        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        Self::verify_domain_owner(&mut transaction, *listing_source_id, *domain_id).await?;
        let url_strings = urls.iter().map(ToString::to_string).collect::<Vec<_>>();
        if let Some((conflicting_url, current_listing_source_id)) =
            Self::conflicting_owner(&mut transaction, *listing_source_id, &url_strings).await?
        {
            return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                url: conflicting_url,
                requested_listing_source_id: *listing_source_id,
                current_listing_source_id,
            });
        }

        let records = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, created, updated) \
             SELECT $1, $2, input.url, input.url_class, NOW(), NOW() \
             FROM UNNEST($3::text[], $4::text[]) AS input(url, url_class) \
             ON CONFLICT (url) DO UPDATE SET \
                 url_class = EXCLUDED.url_class, \
                 domain_id = EXCLUDED.domain_id, \
                 updated = NOW() \
             WHERE listing_source_urls.listing_source_id = EXCLUDED.listing_source_id \
             RETURNING listing_source_id, domain_id, url, url_class, last_scraped_presence, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id)
        .bind(&url_strings)
        .bind(url_classes.iter().map(ToString::to_string).collect::<Vec<_>>())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;

        if records.len() != urls.len() {
            for url in urls {
                if let Some(current_listing_source_id) =
                    Self::owner_for_url(&mut transaction, url).await?
                    && current_listing_source_id != *listing_source_id
                {
                    return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                        url: url.clone(),
                        requested_listing_source_id: *listing_source_id,
                        current_listing_source_id,
                    });
                }
            }
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Protocol(
                    "crawler URL batch returned fewer rows than inputs".to_owned(),
                ),
            });
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(records)
    }

    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        hash: &str,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        sqlx::query_as::<_, SpiderUrlRecord>(
            "UPDATE listing_source_urls \
             SET last_scraped = NOW(), last_scraped_hash = $3, updated = NOW() \
             WHERE listing_source_id = $1 AND url = $2 \
             RETURNING listing_source_id, domain_id, url, url_class, last_scraped_presence, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(hash)
        .fetch_one(&self.pool)
        .await
    }

    async fn set_presence(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        state: &UrlPresence,
    ) -> Result<SpiderUrlRecord, sqlx::Error> {
        sqlx::query_as::<_, SpiderUrlRecord>(
            "UPDATE listing_source_urls \
             SET last_scraped_presence = $3, updated = NOW() \
             WHERE listing_source_id = $1 AND url = $2 \
             RETURNING listing_source_id, domain_id, url, url_class, last_scraped_presence, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(state.to_string())
        .fetch_one(&self.pool)
        .await
    }
}

fn database_error(source: sqlx::Error) -> UrlMetadataRepositoryError {
    UrlMetadataRepositoryError::Database { source }
}
