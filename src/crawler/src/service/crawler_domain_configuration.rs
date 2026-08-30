//! Explicit crawler-local domain ownership configuration.

use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId};
use sqlx::PgPool;

#[derive(Debug, Clone)]
pub struct CrawlerDomainConfiguration {
    pub domain_id: uuid::Uuid,
    pub listing_source_id: ListingSourceId,
    pub domain: Domain,
}

#[derive(Debug, Clone, Copy)]
pub struct CrawlerDomainRemoval {
    pub domain_id: uuid::Uuid,
    pub removed_url_count: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerDomainConfigurationError {
    #[error("crawler-local ListingSource does not exist")]
    ListingSourceNotFound { listing_source_id: ListingSourceId },
    #[error("crawler domain is already owned by another ListingSource")]
    DomainOwnedByAnotherListingSource {
        domain: Domain,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("crawler domain does not belong to ListingSource")]
    DomainNotOwnedByListingSource {
        listing_source_id: ListingSourceId,
        domain_id: uuid::Uuid,
    },
    #[error("crawler domain configuration database failure")]
    Database {
        #[source]
        source: sqlx::Error,
    },
}

#[async_trait]
pub trait CrawlerDomainConfigurationRepository: Send + Sync {
    async fn list_for_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError>;

    async fn register(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError>;

    async fn remove(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: uuid::Uuid,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError>;
}

#[derive(Clone)]
pub struct CrawlerDomainConfigurationRepositoryImpl {
    pool: PgPool,
}

impl CrawlerDomainConfigurationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CrawlerDomainConfigurationRepository for CrawlerDomainConfigurationRepositoryImpl {
    async fn list_for_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError> {
        let rows = sqlx::query_as::<_, (uuid::Uuid, String)>(
            "SELECT domain_id, listing_source_domain \
             FROM listing_source_domains \
             WHERE listing_source_id = $1 \
             ORDER BY listing_source_domain",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;

        rows.into_iter()
            .map(|(domain_id, raw_domain)| {
                Domain::try_from(raw_domain)
                    .map(|domain| CrawlerDomainConfiguration {
                        domain_id,
                        listing_source_id,
                        domain,
                    })
                    .map_err(|source| CrawlerDomainConfigurationError::Database {
                        source: sqlx::Error::Decode(Box::new(source)),
                    })
            })
            .collect()
    }

    async fn register(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError> {
        let listing_source_id_uuid = uuid::Uuid::from(listing_source_id);
        let mut transaction = self.pool.begin().await.map_err(database_error)?;

        let source_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM listing_sources \
                WHERE listing_source_id = $1 FOR KEY SHARE \
             )",
        )
        .bind(listing_source_id_uuid)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !source_exists {
            return Err(CrawlerDomainConfigurationError::ListingSourceNotFound {
                listing_source_id,
            });
        }

        let registered_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO listing_source_domains (listing_source_id, listing_source_domain) \
             VALUES ($1, $2) \
             ON CONFLICT (listing_source_domain) DO UPDATE \
             SET listing_source_domain = EXCLUDED.listing_source_domain \
             WHERE listing_source_domains.listing_source_id = EXCLUDED.listing_source_id \
             RETURNING domain_id",
        )
        .bind(listing_source_id_uuid)
        .bind(domain.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;

        let domain_id = match registered_id {
            Some(domain_id) => domain_id,
            None => {
                let owner = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT listing_source_id FROM listing_source_domains \
                     WHERE listing_source_domain = $1",
                )
                .bind(domain.as_str())
                .fetch_one(&mut *transaction)
                .await
                .map_err(database_error)?;
                return Err(
                    CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource {
                        domain,
                        requested_listing_source_id: listing_source_id,
                        current_listing_source_id: owner.into(),
                    },
                );
            }
        };

        transaction.commit().await.map_err(database_error)?;
        Ok(CrawlerDomainConfiguration {
            domain_id,
            listing_source_id,
            domain,
        })
    }

    async fn remove(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: uuid::Uuid,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let url_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM listing_source_urls \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;

        let removed = sqlx::query(
            "DELETE FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
        if removed.rows_affected() == 0 {
            return Err(
                CrawlerDomainConfigurationError::DomainNotOwnedByListingSource {
                    listing_source_id,
                    domain_id,
                },
            );
        }

        transaction.commit().await.map_err(database_error)?;
        Ok(CrawlerDomainRemoval {
            domain_id,
            removed_url_count: url_count,
        })
    }
}

fn database_error(source: sqlx::Error) -> CrawlerDomainConfigurationError {
    CrawlerDomainConfigurationError::Database { source }
}
