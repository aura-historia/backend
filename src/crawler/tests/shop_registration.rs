use crawler::service::shop_registration::{
    RegisteredShop, ShopRegistrationRepository, ShopRegistrationRepositoryImpl,
};
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use std::collections::HashSet;

use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_shop(shop_id: ShopId, domain: &str) -> RegisteredShop {
    RegisteredShop {
        shop_id,
        shop_name: "Test Shop".to_string(),
        shop_slug: "test-shop".to_string(),
        shop_type: ShopType::CommercialDealer,
        domains: HashSet::from([Domain::try_from(domain).unwrap()]),
    }
}

// ---------------------------------------------------------------------------
// deactivate_shops_not_in — deactivated shop's domains are deleted
// ---------------------------------------------------------------------------

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn deactivate_shops_not_in_should_delete_domains_of_deactivated_shops() {
    let pool = get_postgres_client().await;
    let repo = ShopRegistrationRepositoryImpl::new(pool.clone());

    let kept_id = ShopId::from(uuid::Uuid::new_v4());
    let removed_id = ShopId::from(uuid::Uuid::new_v4());

    // Register both shops with a domain each.
    repo.upsert_shop(&make_shop(kept_id, "kept.example.com"))
        .await
        .unwrap();
    repo.sync_domains(&make_shop(kept_id, "kept.example.com"))
        .await
        .unwrap();

    repo.upsert_shop(&make_shop(removed_id, "removed.example.com"))
        .await
        .unwrap();
    repo.sync_domains(&make_shop(removed_id, "removed.example.com"))
        .await
        .unwrap();

    // Verify both domains exist before deactivation.
    let before: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM shop_domains")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before.0, 2, "both domains should exist before deactivation");

    // Deactivate the removed shop (only kept_id is in the active list).
    let deactivated = repo.deactivate_shops_not_in(&[kept_id]).await.unwrap();

    assert_eq!(deactivated, 1, "exactly one shop should be deactivated");

    // The removed shop's domain must be gone.
    let removed_uuid: uuid::Uuid = removed_id.into();
    let removed_domain_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM shop_domains WHERE shop_id = $1")
            .bind(removed_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        removed_domain_count.0, 0,
        "deactivated shop's domain should be deleted"
    );

    // The kept shop's domain must still be present.
    let kept_uuid: uuid::Uuid = kept_id.into();
    let kept_domain_count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM shop_domains WHERE shop_id = $1")
            .bind(kept_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        kept_domain_count.0, 1,
        "active shop's domain should be preserved"
    );
}

// ---------------------------------------------------------------------------
// deactivate_shops_not_in — shop that stays active keeps its domains
// ---------------------------------------------------------------------------

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn deactivate_shops_not_in_should_not_touch_active_shop_domains() {
    let pool = get_postgres_client().await;
    let repo = ShopRegistrationRepositoryImpl::new(pool.clone());

    let shop_id = ShopId::from(uuid::Uuid::new_v4());

    repo.upsert_shop(&make_shop(shop_id, "active.example.com"))
        .await
        .unwrap();
    repo.sync_domains(&make_shop(shop_id, "active.example.com"))
        .await
        .unwrap();

    // Deactivate with shop_id in the active list → nothing should be deactivated.
    let deactivated = repo.deactivate_shops_not_in(&[shop_id]).await.unwrap();

    assert_eq!(deactivated, 0, "no shop should be deactivated");

    let shop_uuid: uuid::Uuid = shop_id.into();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM shop_domains WHERE shop_id = $1")
        .bind(shop_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1, "active shop's domain should remain untouched");
}

// ---------------------------------------------------------------------------
// deactivate_shops_not_in — returns 0 when no shops need deactivation
// ---------------------------------------------------------------------------

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn deactivate_shops_not_in_should_return_zero_when_nothing_to_deactivate() {
    let pool = get_postgres_client().await;
    let repo = ShopRegistrationRepositoryImpl::new(pool.clone());

    // No shops in DB, any active list → 0 deactivated.
    let deactivated = repo
        .deactivate_shops_not_in(&[ShopId::from(uuid::Uuid::new_v4())])
        .await
        .unwrap();

    assert_eq!(deactivated, 0);
}
