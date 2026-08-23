use aura_historia_worker::{
    QueueConfig, WorkerRunError, WorkerRuntime, cdc::WorkerQueue,
    product_translation::consume_product_translation_queue, serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use platform_postgres::SqlxUnitOfWork;
use product_core::product_id::ProductId;
use product_postgres::{SqlxProductTranslationSourceReader, SqlxProductTranslationWriterFactory};
use product_service::use_cases::{TranslateProductEventHandler, TranslateProductEventUseCase};
use product_translation_llm::LargeLanguageModelProductTitleTranslator;
use std::{sync::Arc, time::Duration};
use test_api::{
    IntegrationTestService, Postgres, Sequin, aura_integration_test, get_postgres_client,
    get_sequin_worker_webhook_bind_addr,
};
use tokio::{sync::oneshot, task::JoinHandle};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

struct FixedTranslationLlm;

#[async_trait::async_trait]
impl LargeLanguageModel for FixedTranslationLlm {
    async fn generate<Output>(
        &self,
        _request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        serde_json::from_str(
            r#"{"titles":{"en":"Antique oak chair","fr":"Chaise ancienne en chêne","es":"Silla antigua de roble","it":"Sedia antica in rovere"}}"#,
        )
        .map_err(|source| LargeLanguageModelError::InvalidResponse {
            source: application::error::box_error(source),
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_translate_committed_embedded_product_event_and_persist_canonical_target_shape() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, _) =
            insert_product_with_event(&worker.pool, "ENRICHMENT_EMBEDDED", "ENRICHMENT").await?;

        let rows = wait_for_translations(&worker.pool, product_id, 4).await?;

        assert_eq!(
            vec![
                ("en".to_owned(), "Antique oak chair".to_owned()),
                ("es".to_owned(), "Silla antigua de roble".to_owned()),
                ("fr".to_owned(), "Chaise ancienne en chêne".to_owned()),
                ("it".to_owned(), "Sedia antica in rovere".to_owned()),
            ],
            rows
        );
        let count = enrichment_event_count(&worker.pool, product_id).await?;
        assert_eq!(
            2, count,
            "source embedded event plus one batch translated event"
        );
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_ignore_non_embedded_product_event_without_translation_side_effect() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, _) =
            insert_product_with_event(&worker.pool, "PRODUCT_STATE_CHANGED", "DOMAIN").await?;

        assert_no_translations(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_not_translate_rolled_back_embedded_product_event() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_id = insert_product_with_event_then_rollback(&worker.pool).await?;

        assert_no_translations(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_skip_stale_embedded_event_after_product_revision_advances() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, embedded_event_id) =
            insert_product_with_event(&worker.pool, "ENRICHMENT_EMBEDDED", "ENRICHMENT").await?;
        let _newer_event_id = advance_product_revision(&worker.pool, product_id).await?;

        worker
            .redeliver(
                product_id,
                embedded_event_id,
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
            )
            .await?;
        assert_no_translations(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_keep_one_translation_batch_when_embedded_event_is_redelivered() {
    let worker = TranslationWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, event_id) =
            insert_product_with_event(&worker.pool, "ENRICHMENT_EMBEDDED", "ENRICHMENT").await?;
        let _rows = wait_for_translations(&worker.pool, product_id, 4).await?;

        worker
            .redeliver(product_id, event_id, "ENRICHMENT_EMBEDDED", "ENRICHMENT")
            .await?;
        assert_translation_count_for_duration(
            &worker.pool,
            product_id,
            4,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await?;
        assert_eq!(2, enrichment_event_count(&worker.pool, product_id).await?);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .expect("worker cleanup or test failed");
}

struct TranslationWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    _unused_receivers: aura_historia_worker::cdc::WorkerQueueReceivers,
    consumer: JoinHandle<()>,
}

impl TranslationWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn TranslateProductEventUseCase> =
            Arc::new(TranslateProductEventHandler::new(
                SqlxProductTranslationSourceReader::new(pool.clone()),
                LargeLanguageModelProductTitleTranslator::new(FixedTranslationLlm),
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductTranslationWriterFactory::new(),
            ));
        let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(16))
            .expect("valid worker queue configuration");
        let receiver = receivers
            .take(WorkerQueue::ProductTranslate)
            .expect("product translation queue is registered");
        let consumer = tokio::spawn(consume_product_translation_queue(receiver, handler));
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr())
            .await
            .expect("worker webhook bind address is available");
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        Self {
            pool,
            shutdown_tx,
            server,
            _unused_receivers: receivers,
            consumer,
        }
    }

    async fn redeliver(
        &self,
        product_id: ProductId,
        event_id: EventId,
        event_type: &str,
        event_group: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response = reqwest::Client::new()
            .post(format!(
                "http://127.0.0.1:{}/cdc/sequin",
                get_sequin_worker_webhook_bind_addr().port()
            ))
            .json(&serde_json::json!({
                "record": {
                    "event_id": event_id.to_string(),
                    "product_id": product_id.to_string(),
                    "event_type": event_type,
                    "event_group": event_group,
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
            return Err(std::io::Error::other("worker did not accept redelivery").into());
        }
        Ok(())
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let TranslationWorker {
            shutdown_tx,
            server,
            _unused_receivers,
            consumer,
            ..
        } = self;
        let shutdown_result = shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker shutdown channel closed"));
        drop(_unused_receivers);
        let (server_result, consumer_result) = tokio::join!(server, consumer);
        shutdown_result?;
        server_result??;
        consumer_result?;
        test_result
    }
}

async fn insert_product_with_event(
    pool: &sqlx::PgPool,
    event_type: &str,
    event_group: &str,
) -> Result<(ProductId, EventId), sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Translation worker shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("translation-worker-shop-{shop_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Antiker Eichenstuhl', 'de', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_id))
        .bind(format!("translation-worker-product-{product_id}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(product_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .bind(event_type)
        .bind(event_group)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_id, event_id))
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'PRODUCT_STATE_CHANGED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE products SET event_id = $1 WHERE product_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductId, sqlx::Error> {
    let product_id = ProductId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Rollback translation shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id)
        .bind(format!("rollback-translation-shop-{shop_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shops_product_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Antiker Eichenstuhl', 'de', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_id))
        .bind(format!("rollback-translation-product-{product_id}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(shop_id)
        .bind(product_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', '{}', now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(product_id)
}

async fn wait_for_translations(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    expected_count: i64,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let rows = translations(pool, product_id).await?;
        if i64::try_from(rows.len())? == expected_count {
            return Ok(rows);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product translations").into())
}

async fn assert_no_translations(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if !translations(pool, product_id).await?.is_empty() {
            return Err(std::io::Error::other("unexpected product translation persisted").into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_translation_count_for_duration(
    pool: &sqlx::PgPool,
    product_id: ProductId,
    expected_count: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if translations(pool, product_id).await?.len() != expected_count {
            return Err(std::io::Error::other(
                "product translation count changed after redelivery",
            )
            .into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn translations(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT language, title FROM product_translations WHERE product_id = $1 ORDER BY language",
    )
    .bind(uuid::Uuid::from(product_id))
    .fetch_all(pool)
    .await
}

async fn enrichment_event_count(
    pool: &sqlx::PgPool,
    product_id: ProductId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM product_events WHERE product_id = $1 AND event_group = 'ENRICHMENT'",
    )
    .bind(uuid::Uuid::from(product_id))
    .fetch_one(pool)
    .await
}
