use aura_historia_worker::{
    QueueConfig, WorkerRunError, WorkerRuntime, cdc::WorkerQueue,
    product_opensearch::consume_product_opensearch_queue, serve_with_runtime,
};
use common::{event_id::EventId, product_id::ProductId};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use opensearch::GetParts;
use platform_postgres::SqlxUnitOfWork;
use product_opensearch::OpenSearchProductSearchProjection;
use product_postgres::SqlxProductSearchFilterMatchSourceReaderFactory;
use product_service::use_cases::{ProjectProductHandler, ProjectProductUseCase};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, Sequin, aura_integration_test,
    get_opensearch_client, get_postgres_client, get_sequin_worker_webhook_bind_addr, refresh_index,
};
use tokio::{sync::oneshot, task::JoinHandle};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const PRODUCTS_INDEX: &str = "products";
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_PROJECTION_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_project_committed_active_product_with_native_source_price_and_no_estimates() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 7).await?;

        let response = wait_for_product_response(fixture.product_id).await?;
        let document = response
            .get("_source")
            .ok_or_else(|| std::io::Error::other("projected Product response has no _source"))?;

        assert_eq!(Some(7), response.get("_version").and_then(Value::as_i64));
        assert_eq!(
            Some(fixture.product_id.to_string().as_str()),
            document.get("productId").and_then(Value::as_str)
        );
        assert_eq!(
            Some(fixture.event_id.to_string().as_str()),
            document.get("eventId").and_then(Value::as_str)
        );
        assert_eq!(
            Some("Active Product OpenSearch chair"),
            document.pointer("/title/text").and_then(Value::as_str)
        );
        assert_eq!(
            Some("EN"),
            document.pointer("/title/language").and_then(Value::as_str)
        );
        assert_eq!(
            Some(&json!({ "amount": 12_345, "currency": "USD" })),
            document.get("sourcePrice")
        );
        assert!(document.get("priceEstimateMin").is_none());
        assert!(document.get("priceEstimateMax").is_none());
        assert!(document.get("salePrices").is_none());
        assert!(document.get("saleFxRateId").is_none());
        assert!(document.get("soldAt").is_none());
        assert_eq!(
            Some("LISTED"),
            document.get("state").and_then(Value::as_str)
        );
        assert_eq!(
            Some("ACTIVE"),
            document.get("lifecycle").and_then(Value::as_str)
        );
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_not_project_rolled_back_product_event() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_id = insert_product_with_event_then_rollback(&worker.pool).await?;

        assert_no_product_projection(product_id, NO_PROJECTION_OBSERVATION).await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_keep_product_projection_unchanged_when_event_is_redelivered() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 9).await?;
        let expected = wait_for_product_response(fixture.product_id).await?;

        worker
            .redeliver(fixture.product_id, fixture.event_id)
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_id,
            &expected,
            NO_PROJECTION_OBSERVATION,
        )
        .await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_skip_stale_product_event_trigger() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 11).await?;
        let _ = wait_for_product_response(fixture.product_id).await?;
        let current_event_id = advance_product_revision(&worker.pool, fixture.product_id).await?;
        let expected = wait_for_product_event(fixture.product_id, current_event_id).await?;

        assert_eq!(
            Some(current_event_id.to_string().as_str()),
            expected.pointer("/_source/eventId").and_then(Value::as_str)
        );
        worker
            .redeliver(fixture.product_id, fixture.event_id)
            .await?;
        assert_product_response_unchanged_for(
            fixture.product_id,
            &expected,
            NO_PROJECTION_OBSERVATION,
        )
        .await
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_remove_indexed_product_when_current_lifecycle_is_deleted() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fixture = insert_active_product_with_event(&worker.pool, 15).await?;
        let response = wait_for_product_response(fixture.product_id).await?;
        assert_eq!(Some(15), response.get("_version").and_then(Value::as_i64));

        let deleted_event_id = delete_product_lifecycle(&worker.pool, fixture.product_id).await?;
        wait_for_product_deletion(fixture.product_id).await?;
        assert_no_product_projection(fixture.product_id, NO_PROJECTION_OBSERVATION).await?;

        let (current_event_id, projection_version, lifecycle): (uuid::Uuid, i64, String) =
            sqlx::query_as(
                "SELECT event_id, projection_version, lifecycle FROM products WHERE product_id = $1",
            )
            .bind(uuid::Uuid::from(fixture.product_id))
            .fetch_one(&worker.pool)
            .await?;
        assert_eq!(uuid::Uuid::from(deleted_event_id), current_event_id);
        assert_eq!(16, projection_version);
        assert_eq!("DELETED", lifecycle);
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_project_sold_product_with_all_sale_price_currencies() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fx_rate_id = insert_equal_rate_snapshot(&worker.pool).await?;
        let fixture = insert_sold_product_with_event(&worker.pool, fx_rate_id).await?;

        let response = wait_for_product_response(fixture.product_id).await?;
        let document = response
            .get("_source")
            .ok_or_else(|| std::io::Error::other("projected Product response has no _source"))?;
        let sale_prices = document
            .get("salePrices")
            .ok_or_else(|| std::io::Error::other("sold Product projection has no salePrices"))?;

        assert_eq!(
            Some(&json!({ "amount": 12_345, "currency": "EUR" })),
            document.get("sourcePrice")
        );
        assert_eq!(
            Some(fx_rate_id.to_string().as_str()),
            document.get("saleFxRateId").and_then(Value::as_str)
        );
        assert!(document.get("soldAt").and_then(Value::as_str).is_some());
        for currency in [
            "eur", "gbp", "usd", "aud", "cad", "nzd", "cny", "brl", "pln", "try", "jpy", "czk",
            "rub", "aed", "sar", "hkd", "sgd", "chf",
        ] {
            let expected = if currency == "jpy" { 123 } else { 12_345 };
            assert_eq!(
                Some(expected),
                sale_prices.get(currency).and_then(Value::as_i64),
                "sale price for {currency}"
            );
        }
        assert_eq!(Some("SOLD"), document.get("state").and_then(Value::as_str));
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_project_sold_product_without_main_price_then_add_sale_prices_when_corrected() {
    let worker = ProductOpenSearchWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let fx_rate_id = insert_equal_rate_snapshot(&worker.pool).await?;
        let fixture =
            insert_sold_product_without_main_price_with_event(&worker.pool, fx_rate_id).await?;

        let initial = wait_for_product_response(fixture.product_id).await?;
        let initial_document = initial
            .get("_source")
            .ok_or_else(|| std::io::Error::other("projected Product response has no _source"))?;
        assert!(initial_document.get("sourcePrice").is_none());
        assert!(initial_document.get("salePrices").is_none());
        assert_eq!(
            Some(fx_rate_id.to_string().as_str()),
            initial_document.get("saleFxRateId").and_then(Value::as_str)
        );
        assert!(
            initial_document
                .get("soldAt")
                .and_then(Value::as_str)
                .is_some()
        );
        assert!(initial_document.get("priceEstimateMin").is_none());
        assert!(initial_document.get("priceEstimateMax").is_none());

        let corrected_event_id =
            correct_sold_product_main_price(&worker.pool, fixture.product_id).await?;
        let corrected = wait_for_product_event(fixture.product_id, corrected_event_id).await?;
        let corrected_document = corrected
            .get("_source")
            .ok_or_else(|| std::io::Error::other("corrected Product response has no _source"))?;
        assert_eq!(
            Some(&json!({ "amount": 12_345, "currency": "EUR" })),
            corrected_document.get("sourcePrice")
        );
        let sale_prices = corrected_document
            .get("salePrices")
            .ok_or_else(|| std::io::Error::other("corrected sold Product has no salePrices"))?;
        for currency in [
            "eur", "gbp", "usd", "aud", "cad", "nzd", "cny", "brl", "pln", "try", "jpy", "czk",
            "rub", "aed", "sar", "hkd", "sgd", "chf",
        ] {
            let expected = if currency == "jpy" { 123 } else { 12_345 };
            assert_eq!(
                Some(expected),
                sale_prices.get(currency).and_then(Value::as_i64)
            );
        }
        Ok(())
    }
    .await;

    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

struct ProductFixture {
    product_id: ProductId,
    event_id: EventId,
}

struct ProductOpenSearchWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl ProductOpenSearchWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn ProjectProductUseCase> = Arc::new(ProjectProductHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductSearchFilterMatchSourceReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductSearchProjection::new(get_opensearch_client().await.clone()),
        ));
        let (runtime, mut receivers) = WorkerRuntime::with_product_opensearch_queue(
            QueueConfig::new(16),
        )
        .unwrap_or_else(|error| panic!("valid Product OpenSearch queue configuration: {error}"));
        let receiver = receivers
            .take(WorkerQueue::ProductOpenSearch)
            .unwrap_or_else(|| panic!("Product OpenSearch queue is registered"));
        let consumer = tokio::spawn(consume_product_opensearch_queue(receiver, handler));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr())
            .await
            .unwrap_or_else(|error| panic!("worker webhook bind address is available: {error}"));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Self {
            pool,
            shutdown_tx,
            server,
            consumer,
        }
    }

    async fn redeliver(
        &self,
        product_id: ProductId,
        event_id: EventId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr().port()
            ))
            .json(&json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_id": product_id.to_string(),
                    "event_type": "PRODUCT_STATE_CHANGED",
                    "event_group": "DOMAIN",
                },
                "action": "insert",
                "metadata": {
                    "table_schema": "public",
                    "table_name": "product_events",
                }
            }))
            .send()
            .await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(
                std::io::Error::other("worker did not accept Product event redelivery").into(),
            );
        }
        Ok(())
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown_result = self
            .shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker shutdown channel closed"));
        let (server_result, consumer_result) = tokio::join!(self.server, self.consumer);
        shutdown_result?;
        server_result??;
        consumer_result?;
        test_result
    }
}

async fn insert_active_product_with_event(
    pool: &sqlx::PgPool,
    projection_version: i64,
) -> Result<ProductFixture, sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    insert_shop(&mut tx, shop_id, "active-os").await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, description_text, description_language, price_amount, price_currency, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, state, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $4, $4, $5, 'Active Product OpenSearch chair', 'en', 'Native source price only', 'en', 12345, 'USD', 10000, 'USD', 15000, 'USD', 'LISTED', 'ACTIVE', 'https://example.test/products/active', '[{\"url\": \"https://example.test/images/active.jpg\", \"prohibited_content\": \"NONE\"}]', $6)",
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(product_slug("active-os", product_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(product_id.to_string())
    .bind(projection_version)
    .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductFixture {
        product_id,
        event_id,
    })
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductId, sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    insert_shop(&mut tx, shop_id, "rollback-os").await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Rolled back Product OpenSearch chair', 'en', 'LISTED', 'ACTIVE', 'https://example.test/products/rolled-back', '[]')",
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(product_slug("rollback-os", product_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(product_id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    tx.rollback().await?;
    Ok(product_id)
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    sqlx::query(
        "UPDATE products SET event_id = $1, title_text = 'Current Product OpenSearch chair', projection_version = projection_version + 1, updated = now() WHERE product_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn delete_product_lifecycle(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    sqlx::query(
        "UPDATE products SET event_id = $1, lifecycle = 'DELETED', projection_version = projection_version + 1, updated = now() WHERE product_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn insert_sold_product_with_event(
    pool: &sqlx::PgPool,
    fx_rate_id: uuid::Uuid,
) -> Result<ProductFixture, sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    insert_shop(&mut tx, shop_id, "sold-os").await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, price_amount, price_currency, sale_fx_rate_id, sold_at, state, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $4, $4, $5, 'Sold Product OpenSearch chair', 'en', 12345, 'EUR', $6, now(), 'SOLD', 'ACTIVE', 'https://example.test/products/sold', '[]', 13)",
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(product_slug("sold-os", product_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(product_id.to_string())
    .bind(fx_rate_id)
    .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductFixture {
        product_id,
        event_id,
    })
}

async fn insert_sold_product_without_main_price_with_event(
    pool: &sqlx::PgPool,
    fx_rate_id: uuid::Uuid,
) -> Result<ProductFixture, sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    insert_shop(&mut tx, shop_id, "sold-no-price-os").await?;
    sqlx::query(
        "INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, sale_fx_rate_id, sold_at, state, lifecycle, url, product_images, projection_version) VALUES ($1, $2, $3, $4, $4, $5, 'Sold Product OpenSearch chair without main price', 'en', 10000, 'EUR', 15000, 'EUR', $6, now(), 'SOLD', 'ACTIVE', 'https://example.test/products/sold-no-price', '[]', 17)",
    )
    .bind(uuid::Uuid::from(product_id))
    .bind(product_slug("sold-no-price-os", product_id))
    .bind(uuid::Uuid::from(event_id))
    .bind(shop_id)
    .bind(product_id.to_string())
    .bind(fx_rate_id)
    .execute(&mut *tx)
    .await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    tx.commit().await?;
    Ok(ProductFixture {
        product_id,
        event_id,
    })
}

async fn correct_sold_product_main_price(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    insert_product_event(&mut tx, product_id, event_id).await?;
    sqlx::query(
        "UPDATE products SET event_id = $1, price_amount = 12345, price_currency = 'EUR', projection_version = projection_version + 1, updated = now() WHERE product_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

fn product_slug(prefix: &str, product_id: ProductId) -> String {
    let product_uuid = uuid::Uuid::from(product_id);
    let suffix = product_uuid.simple().to_string();
    format!("{prefix}-{}", &suffix[..6])
}

async fn insert_shop(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    shop_id: uuid::Uuid,
    slug_prefix: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Product OpenSearch worker shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')",
    )
    .bind(shop_id)
    .bind(format!("{slug_prefix}-{shop_id}"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_product_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product_id: ProductId,
    event_id: EventId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_STATE_CHANGED', 'DOMAIN', '{}', now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_id))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_equal_rate_snapshot(pool: &sqlx::PgPool) -> Result<uuid::Uuid, sqlx::Error> {
    let fx_rate_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), 'fxratesapi', $2)",
    )
    .bind(fx_rate_id)
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await?;
    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, 1000000)",
        )
        .bind(fx_rate_id)
        .bind(currency)
        .execute(pool)
        .await?;
    }
    Ok(fx_rate_id)
}

async fn wait_for_product_response(
    product_id: ProductId,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCTS_INDEX).await;
        if let Some(response) = product_response(product_id).await? {
            return Ok(response);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for Product OpenSearch projection for {product_id}"
    ))
    .into())
}

async fn wait_for_product_event(
    product_id: ProductId,
    event_id: EventId,
) -> Result<Value, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCTS_INDEX).await;
        if let Some(response) = product_response(product_id).await?
            && response.pointer("/_source/eventId").and_then(Value::as_str)
                == Some(event_id.to_string().as_str())
        {
            return Ok(response);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for Product OpenSearch projection revision {event_id}"
    ))
    .into())
}

async fn wait_for_product_deletion(
    product_id: ProductId,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index(PRODUCTS_INDEX).await;
        if product_response(product_id).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "timed out waiting for Product OpenSearch deletion for {product_id}"
    ))
    .into())
}

async fn assert_no_product_projection(
    product_id: ProductId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        refresh_index(PRODUCTS_INDEX).await;
        if product_response(product_id).await?.is_some() {
            return Err(std::io::Error::other(format!(
                "unexpected Product OpenSearch projection for {product_id}"
            ))
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_product_response_unchanged_for(
    product_id: ProductId,
    expected: &Value,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        refresh_index(PRODUCTS_INDEX).await;
        let actual = product_response(product_id).await?.ok_or_else(|| {
            std::io::Error::other("Product projection disappeared after redelivery")
        })?;
        if &actual != expected {
            return Err(
                std::io::Error::other("Product projection changed after redelivery").into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn product_response(
    product_id: ProductId,
) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(PRODUCTS_INDEX, &product_id.to_string()))
        .send()
        .await?;
    if response.status_code().as_u16() == 404 {
        return Ok(None);
    }
    let response = response.error_for_status_code()?;
    Ok(Some(response.json().await?))
}
