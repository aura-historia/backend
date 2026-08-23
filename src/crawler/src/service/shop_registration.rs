//! Service for registering and syncing shops from an external source into the crawler's local DB.
//!
//! # Overview
//!
//! The crawler needs to know which shops exist and which domains they own so it can schedule
//! spider and scraper work. This module provides:
//!
//! - [`ShopRegistrationSource`] — trait for fetching the authoritative Shop summary list.
//! - [`ShopRegistrationRepository`] — trait for persisting shop + domain data to Postgres.
//! - [`ShopRegistrationService`] — orchestrates a full sync cycle: fetch → upsert → deactivate stale.
//! - [`ShopRegistrationRepositoryImpl`] — Postgres-backed repository implementation.
//! - [`RegisteredShop`] — value object carrying shop identity, type, and domains.

use async_trait::async_trait;
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use sqlx::PgPool;
use std::collections::HashSet;
use tracing::{error, info, warn};

/// A shop summary from the authoritative Shop service that is synced into crawler-local Postgres.
#[derive(Debug, Clone)]
pub struct RegisteredShop {
    pub shop_id: ShopId,
    pub shop_name: String,
    pub shop_slug: String,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
}

/// Fetches all registered shops from an external source.
///
/// The crawler crate owns this trait but does **not** provide an implementation —
/// the binary composes the canonical Shop service and PostgreSQL reader.
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

    #[tracing::instrument(name = "shop_registration_sync", skip(self))]
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
                    shop_slug = %shop.shop_slug,
                    error = %e,
                    "Failed to upsert shop during sync"
                );
                continue;
            }

            if let Err(e) = self.repository.sync_domains(shop).await {
                error!(
                    shop_id = %shop.shop_id,
                    shop_name = %shop.shop_name,
                    shop_slug = %shop.shop_slug,
                    domains = shop.domains.len(),
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

        if count > 0 {
            info!(count, "Shop sync complete");
        }
        Ok(count)
    }
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

/// Maps [`ShopType`] to the TEXT representation stored in the `shops` table.
fn shop_type_to_db(shop_type: ShopType) -> &'static str {
    match shop_type {
        ShopType::AuctionHouse => "AUCTION_HOUSE",
        ShopType::AuctionPlatform => "AUCTION_PLATFORM",
        ShopType::CommercialDealer => "COMMERCIAL_DEALER",
        ShopType::Marketplace => "MARKETPLACE",
    }
}

/// Parses the TEXT representation from the `shops` table back to [`ShopType`].
/// Returns `None` for unknown values (e.g. NULL or legacy data).
pub fn shop_type_from_db(raw: Option<&str>) -> Option<ShopType> {
    match raw? {
        "AUCTION_HOUSE" => Some(ShopType::AuctionHouse),
        "AUCTION_PLATFORM" => Some(ShopType::AuctionPlatform),
        "COMMERCIAL_DEALER" => Some(ShopType::CommercialDealer),
        "MARKETPLACE" => Some(ShopType::Marketplace),
        _ => None,
    }
}

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
        let shop_type_str = shop_type_to_db(shop.shop_type);

        // Upsert the shop row with name, slug, and type
        sqlx::query(
            "INSERT INTO shops (shop_id, shop_name, shop_slug, shop_type, active, created, updated)
             VALUES ($1, $2, $3, $4, TRUE, NOW(), NOW())
             ON CONFLICT (shop_id)
             DO UPDATE SET
                  shop_name = EXCLUDED.shop_name,
                  shop_slug = EXCLUDED.shop_slug,
                  shop_type = EXCLUDED.shop_type,
                  active = TRUE,
                  updated = NOW()",
        )
        .bind(shop_id_uuid)
        .bind(&shop.shop_name)
        .bind(&shop.shop_slug)
        .bind(shop_type_str)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn sync_domains(&self, shop: &RegisteredShop) -> Result<(), sqlx::Error> {
        let shop_id_uuid: uuid::Uuid = shop.shop_id.into();
        let domain_strings: Vec<String> = shop
            .domains
            .iter()
            .map(|d| d.as_str().to_string())
            .collect();

        let mut tx = self.pool.begin().await?;

        if domain_strings.is_empty() {
            sqlx::query("DELETE FROM shop_domains WHERE shop_id = $1")
                .bind(shop_id_uuid)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(());
        }

        // Bulk upsert domains. Only reset crawl state when ownership changes.
        sqlx::query(
            "INSERT INTO shop_domains (shop_id, shop_domain, last_crawled)
             SELECT $1, domain, NULL
             FROM unnest($2::text[]) AS t(domain)
             ON CONFLICT (shop_domain)
             DO UPDATE SET
                 shop_id = EXCLUDED.shop_id,
                 last_crawled = CASE
                     WHEN shop_domains.shop_id <> EXCLUDED.shop_id THEN NULL
                     ELSE shop_domains.last_crawled
                 END",
        )
        .bind(shop_id_uuid)
        .bind(&domain_strings)
        .execute(&mut *tx)
        .await?;

        // Remove stale domains that are no longer present in upstream for this shop.
        sqlx::query(
            "DELETE FROM shop_domains
             WHERE shop_id = $1
               AND NOT (shop_domain = ANY($2::text[]))",
        )
        .bind(shop_id_uuid)
        .bind(&domain_strings)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

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

        let mut tx = self.pool.begin().await?;

        // Deactivate shops not in the active set and collect their IDs.
        let deactivated: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "UPDATE shops
             SET active = FALSE,
                 updated = NOW()
             WHERE active = TRUE
               AND NOT (shop_id = ANY($1::uuid[]))
             RETURNING shop_id",
        )
        .bind(&active_ids)
        .fetch_all(&mut *tx)
        .await?;

        let deactivated_count = deactivated.len() as u64;

        if deactivated_count > 0 {
            let deactivated_ids: Vec<uuid::Uuid> =
                deactivated.into_iter().map(|(id,)| id).collect();

            // Remove all domain rows for shops that are no longer active so
            // the spider candidate query never picks them up.
            sqlx::query("DELETE FROM shop_domains WHERE shop_id = ANY($1::uuid[])")
                .bind(&deactivated_ids)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;

        Ok(deactivated_count)
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
                        shop_type: ShopType::CommercialDealer,
                        domains: HashSet::from([Domain::try_from("example.com").unwrap()]),
                    },
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "Another Shop".to_string(),
                        shop_slug: "another-shop".to_string(),
                        shop_type: ShopType::AuctionHouse,
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
                        shop_type: ShopType::CommercialDealer,
                        domains: HashSet::from([Domain::try_from("fail.com").unwrap()]),
                    },
                    RegisteredShop {
                        shop_id: ShopId::new(),
                        shop_name: "OK Shop".to_string(),
                        shop_slug: "ok-shop".to_string(),
                        shop_type: ShopType::CommercialDealer,
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
                    shop_type: ShopType::CommercialDealer,
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

    #[tokio::test]
    async fn should_convert_shop_type_to_db_string() {
        assert_eq!(shop_type_to_db(ShopType::AuctionHouse), "AUCTION_HOUSE");
        assert_eq!(
            shop_type_to_db(ShopType::AuctionPlatform),
            "AUCTION_PLATFORM"
        );
        assert_eq!(
            shop_type_to_db(ShopType::CommercialDealer),
            "COMMERCIAL_DEALER"
        );
        assert_eq!(shop_type_to_db(ShopType::Marketplace), "MARKETPLACE");
    }

    #[tokio::test]
    async fn should_parse_shop_type_from_db_string() {
        assert_eq!(
            shop_type_from_db(Some("AUCTION_HOUSE")),
            Some(ShopType::AuctionHouse)
        );
        assert_eq!(
            shop_type_from_db(Some("AUCTION_PLATFORM")),
            Some(ShopType::AuctionPlatform)
        );
        assert_eq!(
            shop_type_from_db(Some("COMMERCIAL_DEALER")),
            Some(ShopType::CommercialDealer)
        );
        assert_eq!(
            shop_type_from_db(Some("MARKETPLACE")),
            Some(ShopType::Marketplace)
        );
        assert_eq!(shop_type_from_db(Some("unknown")), None);
        assert_eq!(shop_type_from_db(None), None);
    }
}
