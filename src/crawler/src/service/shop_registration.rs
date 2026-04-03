use async_trait::async_trait;
use common::domain::Domain;
use common::shop_id::ShopId;
use sqlx::PgPool;
use std::collections::HashSet;
use tracing::{error, info, warn};

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
    async fn sync_domains(&self, shop: &RegisteredShop) -> Result<(), sqlx::Error>;
    async fn deactivate_shops_not_in(&self, active_shop_ids: &[ShopId])
    -> Result<u64, sqlx::Error>;
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

        if shops.is_empty() {
            warn!("Shop sync returned 0 shops; skipping deactivation pass");
            return Ok(0);
        }

        let count = shops.len();
        let mut active_shop_ids = Vec::with_capacity(count);

        for shop in &shops {
            active_shop_ids.push(shop.shop_id);

            if let Err(e) = self.repository.upsert_shop(shop).await {
                error!(
                    shop_id = %shop.shop_id,
                    shop_name = %shop.shop_name,
                    error = %e,
                    "Failed to upsert shop during sync"
                );
                continue;
            }

            if let Err(e) = self.repository.sync_domains(shop).await {
                error!(
                    shop_id = %shop.shop_id,
                    shop_name = %shop.shop_name,
                    error = %e,
                    "Failed to sync domains during shop sync"
                );
            }
        }

        match self
            .repository
            .deactivate_shops_not_in(&active_shop_ids)
            .await
        {
            Ok(deactivated_count) => {
                if deactivated_count > 0 {
                    info!(
                        deactivated_count,
                        "Deactivated shops not found in upstream sync"
                    );
                }
            }
            Err(e) => error!(error = %e, "Failed to deactivate shops not present in upstream sync"),
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
            "INSERT INTO shops (shop_id, shop_name, shop_slug, active, created, updated)
             VALUES ($1, $2, $3, TRUE, NOW(), NOW())
             ON CONFLICT (shop_id)
             DO UPDATE SET
                  shop_name = EXCLUDED.shop_name,
                  shop_slug = EXCLUDED.shop_slug,
                  active = TRUE,
                  updated = NOW()",
        )
        .bind(shop_id_uuid)
        .bind(&shop.shop_name)
        .bind(&shop.shop_slug)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn sync_domains(&self, shop: &RegisteredShop) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = shop.shop_id.into();

        // Upsert each domain. Reassign moved domains to this shop and reset crawl/lock state.
        for domain in &shop.domains {
            sqlx::query(
                "INSERT INTO shop_domains (shop_id, shop_domain, last_crawled, locked_at)
                 VALUES ($1, $2, NULL, NULL)
                 ON CONFLICT (shop_domain)
                 DO UPDATE SET
                     shop_id = EXCLUDED.shop_id,
                     last_crawled = NULL,
                     locked_at = NULL
                 WHERE shop_domains.shop_id <> EXCLUDED.shop_id",
            )
            .bind(shop_id_uuid)
            .bind(domain.as_str())
            .execute(&self.pool)
            .await?;
        }

        let domain_strings: Vec<String> = shop
            .domains
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();

        if domain_strings.is_empty() {
            sqlx::query("DELETE FROM shop_domains WHERE shop_id = $1")
                .bind(shop_id_uuid)
                .execute(&self.pool)
                .await?;
            return Ok(());
        }

        // Remove stale domains that are no longer present in upstream for this shop.
        sqlx::query(
            "DELETE FROM shop_domains
             WHERE shop_id = $1
               AND NOT (shop_domain = ANY($2::text[]))",
        )
        .bind(shop_id_uuid)
        .bind(domain_strings)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn deactivate_shops_not_in(
        &self,
        active_shop_ids: &[ShopId],
    ) -> Result<u64, sqlx::Error> {
        if active_shop_ids.is_empty() {
            return Ok(0);
        }

        let active_ids: Vec<uuid::Uuid> = active_shop_ids
            .iter()
            .map(|shop_id| (*shop_id).into())
            .collect();

        let result = sqlx::query(
            "UPDATE shops
             SET active = FALSE,
                 updated = NOW()
             WHERE active = TRUE
               AND NOT (shop_id = ANY($1::uuid[]))",
        )
        .bind(active_ids)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
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
        repository
            .expect_sync_domains()
            .times(2)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_deactivate_shops_not_in()
            .times(1)
            .withf(|shop_ids| shop_ids.len() == 2)
            .returning(|_| Box::pin(async { Ok(0) }));

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
        repository
            .expect_sync_domains()
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_deactivate_shops_not_in()
            .times(1)
            .withf(|shop_ids| shop_ids.len() == 2)
            .returning(|_| Box::pin(async { Ok(0) }));

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

    #[tokio::test]
    async fn should_skip_deactivation_when_source_returns_empty() {
        let mut source = MockShopRegistrationSource::new();
        source
            .expect_fetch_registered_shops()
            .returning(|| Box::pin(async { Ok(vec![]) }));

        let repository = MockShopRegistrationRepository::new();

        let service = ShopRegistrationService::new(Box::new(source), Box::new(repository));
        let count = service.sync().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn should_continue_when_domain_sync_fails() {
        let mut source = MockShopRegistrationSource::new();
        source.expect_fetch_registered_shops().returning(|| {
            Box::pin(async {
                Ok(vec![RegisteredShop {
                    shop_id: ShopId::new(),
                    shop_name: "Test Shop".to_string(),
                    shop_slug: "test-shop".to_string(),
                    domains: HashSet::from([Domain::try_from("example.com").unwrap()]),
                }])
            })
        });

        let mut repository = MockShopRegistrationRepository::new();
        repository
            .expect_upsert_shop()
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_sync_domains()
            .times(1)
            .returning(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));
        repository
            .expect_deactivate_shops_not_in()
            .times(1)
            .withf(|shop_ids| shop_ids.len() == 1)
            .returning(|_| Box::pin(async { Ok(0) }));

        let service = ShopRegistrationService::new(Box::new(source), Box::new(repository));
        let count = service.sync().await.unwrap();
        assert_eq!(count, 1);
    }
}
