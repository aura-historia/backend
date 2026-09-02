use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingCurrentEventGuardFactory;
use product_listing_service::ports::{
    ProductListingCurrentEventCheck, ProductListingCurrentEventGuard,
    ProductListingCurrentEventGuardFactory, ProductListingCurrentEventRef,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_block_product_current_event_update_until_current_event_guard_transaction_commits() {
    let result = current_event_guard_lock_flow().await;
    assert!(
        result.is_ok(),
        "current event guard lock integration test failed: {result:?}"
    );
}

async fn current_event_guard_lock_flow() -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let (product_listing_id, current_event_id) = seed_product(&pool).await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let mut guard_transaction = unit_of_work.begin().await?;

    let current_ref = ProductListingCurrentEventRef {
        product_listing_id,
        expected_event_id: current_event_id,
    };
    let stale_ref = ProductListingCurrentEventRef {
        product_listing_id,
        expected_event_id: EventId::new(),
    };
    let current_events = SqlxProductListingCurrentEventGuardFactory::new()
        .in_transaction(&mut guard_transaction)
        .lock_and_check_all(&[current_ref, stale_ref])
        .await?;
    assert_eq!(
        Some(&ProductListingCurrentEventCheck::Current),
        current_events.get(&current_ref)
    );
    assert_eq!(
        Some(&ProductListingCurrentEventCheck::Stale),
        current_events.get(&stale_ref)
    );

    let next_event_id = EventId::new();
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())",
    )
    .bind(uuid::Uuid::from(next_event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(serde_json::json!({
        "availability": {"previous": "AVAILABLE", "current": "SOLD_OUT"}
    }))
    .execute(&pool)
    .await?;
    let (update_started_tx, update_started_rx) = oneshot::channel();
    let update_pool = pool.clone();
    let mut update = tokio::spawn(async move {
        let _ = update_started_tx.send(());
        sqlx::query(
            "UPDATE product_listings SET current_event_id = $1, availability = 'SOLD_OUT', version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2",
        )
        .bind(uuid::Uuid::from(next_event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&update_pool)
        .await
    });
    update_started_rx.await?;

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut update)
            .await
            .is_err(),
        "a ProductListing update committed while the current event guard share lock was held"
    );

    guard_transaction.commit().await?;
    let update_result: Result<sqlx::postgres::PgQueryResult, sqlx::Error> = update.await?;
    update_result?;
    Ok(())
}

async fn seed_product(pool: &sqlx::PgPool) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Current event guard party')",
    )
    .bind(party_id)
    .bind(format!("current-event-guard-party-{party_id}"))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Current event guard source', $3)",
    )
    .bind(listing_source_id)
    .bind(format!("current-event-guard-source-{listing_source_id}"))
    .bind(party_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Current event guard description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')",
    )
    .bind(product_uuid)
    .bind(format!("current-event-guard-{slug_suffix}"))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id)
    .bind(product_uuid.to_string())
    .bind("Current event guard product")
        .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(product_uuid)
    .bind(serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": product_uuid.to_string(),
        "title": {"language": "en", "text": "Current event guard product"},
        "description": {"language": "en", "text": "Current event guard description"},
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE",
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": {"start": null, "end": null}
    }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok((product_listing_id, event_id))
}
