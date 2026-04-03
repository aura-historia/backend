use async_trait::async_trait;
use common::domain::Domain;
use common::shop_id::ShopId;
use sqlx::PgPool;
use std::collections::HashSet;
use tracing::{error, info};

/// A shop registered in the upstream system (e.g. the shop service backed by OpenSearch/DynamoDB)
/// that needs to be synced into the crawler's local Postgres database.
#[derive(Debug, Clone)]
pub struct RegisteredShop {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub shop_slug: String,
    pub domains: HashSet<Domain>,
}

/// Fetches all registered shops from an external source.
///
/// The crawler crate owns this trait but does **not** provide an implementation —
/// that lives at the binary level (e.g. `server.rs`) where the concrete shop service
/// (OpenSearch, DynamoDB, etc.) is available.
#[async_trait]
#[mockall::automock]
pub trait ShopRegistrationSource: Send + Sync {
    async fn fetch_registered_shops(&self) -> Result<Vec<RegisteredShop>, ShopSyncError>;
}

/// Persists registered shops into the crawler's local database.
#[async_trait]
#[mockall::automock]
pub trait ShopRegistrationRepository: Send + Sync {
    async fn upsert_shop(&self, shop: &RegisteredShop) -> Result<(), sqlx::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ShopSyncError {
    #[error("Failed to fetch registered shops: {0}")]
    FetchError(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

/// Orchestrates the shop sync: fetches from the source, upserts into the crawler DB.
pub struct ShopRegistrationService {
    source: Box<dyn ShopRegistrationSource>,
    repository: Box<dyn ShopRegistrationRepository>,
}

impl ShopRegistrationService {
    pub fn new(
        source: Box<dyn ShopRegistrationSource>,
        repository: Box<dyn ShopRegistrationRepository>,
    ) -> Self {
        Self { source, repository }
    }

    pub async fn sync(&self) -> Result<usize, ShopSyncError> {
        let shops = self.source.fetch_registered_shops().await?;
        let count = shops.len();

        for shop in &shops {
            if let Err(e) = self.repository.upsert_shop(shop).await {
                error!(
                    shop_id = %shop.shop_id,
                    shop_name = %shop.shop_name,
                    error = %e,
                    "Failed to upsert shop during sync"
                );
            }
        }

        info!(count, "Shop sync complete");
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

pub struct ShopRegistrationRepositoryImpl {
    pool: PgPool,
}

impl ShopRegistrationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ShopRegistrationRepository for ShopRegistrationRepositoryImpl {
    async fn upsert_shop(&self, shop: &RegisteredShop) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = shop.shop_id.into();

        // Upsert the shop row with name and slug
        sqlx::query(
            "INSERT INTO shops (shop_id, shop_name, shop_slug, created, updated)
             VALUES ($1, $2, $3, NOW(), NOW())
             ON CONFLICT (shop_id)
             DO UPDATE SET
                 shop_name = EXCLUDED.shop_name,
                 shop_slug = EXCLUDED.shop_slug,
                 updated = NOW()",
        )
        .bind(shop_id_uuid)
        .bind(&shop.shop_name)
        .bind(&shop.shop_slug)
        .execute(&self.pool)
        .await?;

        // Upsert each domain
        for domain in &shop.domains {
            sqlx::query(
                "INSERT INTO shop_domains (shop_id, shop_domain)
                 VALUES ($1, $2)
                 ON CONFLICT (shop_domain) DO NOTHING",
            )
            .bind(shop_id_uuid)
            .bind(domain.as_str())
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn should_sync_shops_from_source() {
        let mut source = MockShopRegistrationSource::new();
        source.expect_fetch_registered_shops().returning(|| {
            Box::pin(async {
                Ok(vec![
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "Test Shop".to_string(),
                        shop_slug: "test-shop".to_string(),
                        domains: HashSet::from([Domain::try_from("example.com").unwrap()]),
                    },
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "Another Shop".to_string(),
                        shop_slug: "another-shop".to_string(),
                        domains: HashSet::from([Domain::try_from("another.com").unwrap()]),
                    },
                ])
            })
        });

        let mut repository = MockShopRegistrationRepository::new();
        repository
            .expect_upsert_shop()
            .times(2)
            .returning(|_| Box::pin(async { Ok(()) }));

        let service = ShopRegistrationService::new(Box::new(source), Box::new(repository));
        let count = service.sync().await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn should_continue_on_individual_upsert_failure() {
        let mut source = MockShopRegistrationSource::new();
        source.expect_fetch_registered_shops().returning(|| {
            Box::pin(async {
                Ok(vec![
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "Failing Shop".to_string(),
                        shop_slug: "failing-shop".to_string(),
                        domains: HashSet::from([Domain::try_from("fail.com").unwrap()]),
                    },
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "OK Shop".to_string(),
                        shop_slug: "ok-shop".to_string(),
                        domains: HashSet::from([Domain::try_from("ok.com").unwrap()]),
                    },
                ])
            })
        });

        let mut repository = MockShopRegistrationRepository::new();
        let mut call_count = 0u32;
        repository
            .expect_upsert_shop()
            .times(2)
            .returning(move |_| {
                call_count += 1;
                let fail = call_count == 1;
                Box::pin(async move {
                    if fail {
                        Err(sqlx::Error::RowNotFound)
                    } else {
                        Ok(())
                    }
                })
            });

        let service = ShopRegistrationService::new(Box::new(source), Box::new(repository));
        // sync returns Ok even if individual upserts fail — it logs errors and continues
        let count = service.sync().await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn should_propagate_source_fetch_error() {
        let mut source = MockShopRegistrationSource::new();
        source.expect_fetch_registered_shops().returning(|| {
            Box::pin(async { Err(ShopSyncError::FetchError("connection refused".to_string())) })
        });

        let repository = MockShopRegistrationRepository::new();

        let service = ShopRegistrationService::new(Box::new(source), Box::new(repository));
        let result = service.sync().await;
        assert!(result.is_err());
    }
}
