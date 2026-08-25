use std::time::Duration;

use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::{QueueConfig, WorkerRuntime};
use test_api::*;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_deliver_product_event_change_to_worker_queues() {
    let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(8)).unwrap();
    let listener = TcpListener::bind(get_sequin_worker_webhook_bind_addr())
        .await
        .unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(aura_historia_worker::serve_with_runtime(
        listener,
        runtime,
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    let pool = get_postgres_client().await;
    insert_product_event_under_test(&pool).await;

    let product_job = recv_or_fail(&mut receivers, WorkerQueue::ProductListingOpenSearch).await;
    let percolator_job = recv_or_fail(&mut receivers, WorkerQueue::SearchFilterPercolator).await;

    let _send_result = shutdown_tx.send(());
    server.await.unwrap().unwrap();

    assert!(product_job.is_some());
    assert!(percolator_job.is_some());
}

async fn insert_product_event_under_test(pool: &sqlx::PgPool) {
    let product_id = uuid::Uuid::new_v4();
    let event_id = uuid::Uuid::new_v4();
    let shop_id = uuid::Uuid::new_v4();
    let mut transaction = pool.begin().await.unwrap_or_else(|error| {
        panic!("failed to begin product-event fixture transaction: {error}")
    });

    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Sequin test shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("sequin-test-shop-{shop_id}"))
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test shop: {error}"));
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Sequin test product', 'en', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_id)
        .bind(format!("sequin-test-product-{product_id}"))
        .bind(event_id)
        .bind(shop_id)
        .bind(product_id.to_string())
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test product: {error}"));
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_CREATED', 'DOMAIN', '{}', now())")
        .bind(event_id)
        .bind(product_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to insert Sequin test product event: {error}"));
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit Sequin test product event: {error}"));
}

async fn recv_or_fail(
    receivers: &mut aura_historia_worker::cdc::WorkerQueueReceivers,
    queue: WorkerQueue,
) -> Option<aura_historia_worker::cdc::DomainJob> {
    match receivers.recv_timeout(queue, Duration::from_secs(60)).await {
        Ok(job) => job,
        Err(error) => panic!("timed out waiting for Sequin job on {queue:?}: {error}"),
    }
}
