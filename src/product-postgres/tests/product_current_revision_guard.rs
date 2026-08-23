use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_core::product_id::ProductId;
use product_postgres::SqlxProductCurrentRevisionGuardFactory;
use product_service::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionGuard, ProductCurrentRevisionGuardFactory,
    ProductCurrentRevisionRef,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_block_product_revision_update_until_current_revision_guard_transaction_commits() {
    let result = current_revision_guard_lock_flow().await;
    assert!(
        result.is_ok(),
        "current revision guard lock integration test failed: {result:?}"
    );
}

async fn current_revision_guard_lock_flow() -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let (product_id, current_event_id) = seed_product(&pool).await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut guard_transaction = unit_of_work.begin().await?;

    let current_ref = ProductCurrentRevisionRef {
        product_id,
        expected_event_id: current_event_id,
    };
    let stale_ref = ProductCurrentRevisionRef {
        product_id,
        expected_event_id: EventId::new(),
    };
    let revisions = SqlxProductCurrentRevisionGuardFactory::new()
        .in_transaction(&mut guard_transaction)
        .lock_and_check_all(&[current_ref, stale_ref])
        .await?;
    assert_eq!(
        Some(&ProductCurrentRevisionCheck::Current),
        revisions.get(&current_ref)
    );
    assert_eq!(
        Some(&ProductCurrentRevisionCheck::Stale),
        revisions.get(&stale_ref)
    );

    let next_event_id = EventId::new();
    sqlx::query(
        "INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_UPDATED', 'DOMAIN', '{}', now())",
    )
    .bind(uuid::Uuid::from(next_event_id))
    .bind(uuid::Uuid::from(product_id))
    .execute(&pool)
    .await?;
    let (update_started_tx, update_started_rx) = oneshot::channel();
    let update_pool = pool.clone();
    let mut update = tokio::spawn(async move {
        let _ = update_started_tx.send(());
        sqlx::query("UPDATE products SET event_id = $1 WHERE product_id = $2")
            .bind(uuid::Uuid::from(next_event_id))
            .bind(uuid::Uuid::from(product_id))
            .execute(&update_pool)
            .await
    });
    update_started_rx.await?;

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut update)
            .await
            .is_err(),
        "a Product update committed while the revision guard share lock was held"
    );

    guard_transaction.commit().await?;
    let update_result: Result<sqlx::postgres::PgQueryResult, sqlx::Error> = update.await?;
    update_result?;
    Ok(())
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<(ProductId, EventId), sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let product_uuid = uuid::Uuid::from(product_id);
    let slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, $3, 'COMMERCIAL_DEALER', 'SCRAPED', '{}')",
    )
    .bind(shop_id)
    .bind(format!("revision-guard-shop-{shop_id}"))
    .bind("Revision guard shop")
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, description_text, description_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, $6, 'en', 'Revision guard description', 'en', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')",
    )
    .bind(product_uuid)
    .bind(format!("revision-guard-product-{slug_suffix}"))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(product_uuid.to_string())
    .bind("Revision guard product")
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', 'DOMAIN', '{}', now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(product_uuid)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((product_id, event_id))
}
