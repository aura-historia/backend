use aura_historia_worker::{
    QueueConfig, WorkerRunError, WorkerRuntime, cdc::WorkerQueue,
    product_embedding::consume_product_embedding_queue, serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::{
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    EmbedProductListingEventHandler, EmbedProductListingEventUseCase,
};
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

struct FixedEmbeddingGenerator;

#[async_trait::async_trait]
impl EmbeddingGenerator for FixedEmbeddingGenerator {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        Err(EmbeddingError::InvalidInput {
            reason: "expected product embedding",
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_embed_committed_created_product_event_and_persist_canonical_target_shape() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, source_event_id) = insert_product_with_event(&worker.pool, "DOMAIN_CREATED", "DOMAIN").await?;
        let (embedding, current_event_id) = wait_for_embedding(&worker.pool, product_id).await?;
        assert_eq!(EMBEDDING_DIMENSIONS, embedding.len());
        assert!((embedding[0] - (1.0 / (EMBEDDING_DIMENSIONS as f32).sqrt())).abs() < 0.000_001);
        assert_ne!(uuid::Uuid::from(source_event_id), current_event_id);
        let (source, language, text, event_embedding_length): (String, String, String, i32) = sqlx::query_as(
            "SELECT payload ->> 'sourceEventId', payload -> 'title' ->> 'language', payload -> 'title' ->> 'text', jsonb_array_length(payload -> 'embedding') FROM product_events WHERE product_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'",
        ).bind(uuid::Uuid::from(product_id)).fetch_one(&worker.pool).await?;
        assert_eq!(source_event_id.to_string(), source);
        assert_eq!("de", language);
        assert_eq!("Antiker Eichenstuhl", text);
        assert_eq!(EMBEDDING_DIMENSIONS as i32, event_embedding_length);
        Ok(())
    }.await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_ignore_non_created_product_event_without_embedding_side_effect() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, _) =
            insert_product_with_event(&worker.pool, "DOMAIN_STATE_CHANGED", "DOMAIN").await?;
        assert_no_embedding(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_not_embed_rolled_back_created_product_event() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let product_id = insert_product_with_event_then_rollback(&worker.pool).await?;
        assert_no_embedding(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_skip_stale_created_event_after_product_revision_advances() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, source_event_id) =
            insert_product_with_event(&worker.pool, "DOMAIN_CREATED", "DOMAIN").await?;
        advance_product_revision(&worker.pool, product_id).await?;
        worker
            .redeliver(product_id, source_event_id, "DOMAIN_CREATED", "DOMAIN")
            .await?;
        assert_no_embedding(&worker.pool, product_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_keep_one_embedded_event_when_created_event_is_redelivered() {
    let worker = EmbeddingWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_id, event_id) =
            insert_product_with_event(&worker.pool, "DOMAIN_CREATED", "DOMAIN").await?;
        let _ = wait_for_embedding(&worker.pool, product_id).await?;
        worker
            .redeliver(product_id, event_id, "DOMAIN_CREATED", "DOMAIN")
            .await?;
        assert_embedding_event_count_for_duration(
            &worker.pool,
            product_id,
            1,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

struct EmbeddingWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    unused_receivers: aura_historia_worker::cdc::WorkerQueueReceivers,
    consumer: JoinHandle<()>,
}

impl EmbeddingWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn EmbedProductListingEventUseCase> =
            Arc::new(EmbedProductListingEventHandler::new(
                SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
                FixedEmbeddingGenerator,
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingEmbeddingWriterFactory::new(),
            ));
        let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(16))
            .unwrap_or_else(|error| panic!("valid worker queue configuration: {error}"));
        let receiver = receivers
            .take(WorkerQueue::ProductListingEmbed)
            .unwrap_or_else(|| panic!("product embedding queue is registered"));
        let consumer = tokio::spawn(consume_product_embedding_queue(receiver, handler));
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
            unused_receivers: receivers,
            consumer,
        }
    }

    async fn redeliver(
        &self,
        product_id: ProductListingId,
        event_id: EventId,
        event_type: &str,
        event_group: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let response = reqwest::Client::new().post(format!("http://127.0.0.1:{}/cdc/sequin", get_sequin_worker_webhook_bind_addr().port()))
            .json(&serde_json::json!({"record":{"event_id":event_id.to_string(),"product_id":product_id.to_string(),"event_type":event_type,"event_group":event_group},"action":"insert","metadata":{"table_schema":"public","table_name":"product_events"}}))
            .send().await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(std::io::Error::other("worker did not accept redelivery").into());
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
        drop(self.unused_receivers);
        let (server_result, consumer_result) = tokio::join!(self.server, self.consumer);
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
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_id = ProductListingId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Embedding worker shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id).bind(format!("embedding-worker-shop-{shop_id}")).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shop_listing_id, title_text, title_language, description_text, description_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'LISTED', 'ACTIVE', 'https://example.test/product', '[{\"url\": \"https://example.test/image.jpg\", \"prohibited_content\": \"NONE\"}]')")
        .bind(uuid::Uuid::from(product_id)).bind(format!("embedding-worker-product-{product_id}")).bind(uuid::Uuid::from(event_id)).bind(shop_id).bind(product_id.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, $3, $4, '{}', now())")
        .bind(uuid::Uuid::from(event_id)).bind(uuid::Uuid::from(product_id)).bind(event_type).bind(event_group).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product_id, event_id))
}

async fn insert_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
) -> Result<ProductListingId, sqlx::Error> {
    let product_id = ProductListingId::new();
    let event_id = EventId::new();
    let shop_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO shops (shop_id, shop_slug_id, name, shop_type, partner_status, shop_domains) VALUES ($1, $2, 'Rollback embedding shop', 'COMMERCIAL_DEALER', 'SCRAPED', '{}')")
        .bind(shop_id).bind(format!("rollback-embedding-shop-{shop_id}")).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO products (product_id, product_slug_id, event_id, shop_id, seller_id, shop_listing_id, title_text, title_language, state, lifecycle, url, product_images) VALUES ($1, $2, $3, $4, $4, $5, 'Antiker Eichenstuhl', 'de', 'LISTED', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_id)).bind(format!("rollback-embedding-product-{product_id}")).bind(uuid::Uuid::from(event_id)).bind(shop_id).bind(product_id.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'DOMAIN_CREATED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id)).bind(uuid::Uuid::from(product_id)).execute(&mut *tx).await?;
    tx.rollback().await?;
    Ok(product_id)
}

async fn advance_product_revision(
    pool: &sqlx::PgPool,
    product_id: ProductListingId,
) -> Result<(), sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO product_events (event_id, product_id, event_type, event_group, payload, event_time) VALUES ($1, $2, 'DOMAIN_STATE_CHANGED', 'DOMAIN', '{}', now())")
        .bind(uuid::Uuid::from(event_id)).bind(uuid::Uuid::from(product_id)).execute(&mut *tx).await?;
    sqlx::query("UPDATE products SET event_id = $1 WHERE product_id = $2")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn wait_for_embedding(
    pool: &sqlx::PgPool,
    product_id: ProductListingId,
) -> Result<(Vec<f32>, uuid::Uuid), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let row: (Option<Vec<f32>>, uuid::Uuid) =
            sqlx::query_as("SELECT embedding, event_id FROM products WHERE product_id = $1")
                .bind(uuid::Uuid::from(product_id))
                .fetch_one(pool)
                .await?;
        if let Some(embedding) = row.0 {
            return Ok((embedding, row.1));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product embedding").into())
}

async fn assert_no_embedding(
    pool: &sqlx::PgPool,
    product_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_events WHERE product_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_id)).fetch_one(pool).await?;
        let embedding: Option<Vec<f32>> =
            sqlx::query_scalar("SELECT embedding FROM products WHERE product_id = $1")
                .bind(uuid::Uuid::from(product_id))
                .fetch_optional(pool)
                .await?
                .flatten();
        if count != 0 || embedding.is_some() {
            return Err(std::io::Error::other("unexpected product embedding persisted").into());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assert_embedding_event_count_for_duration(
    pool: &sqlx::PgPool,
    product_id: ProductListingId,
    expected_count: i64,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_events WHERE product_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'").bind(uuid::Uuid::from(product_id)).fetch_one(pool).await?;
        if count != expected_count {
            return Err(
                std::io::Error::other("embedded event count changed after redelivery").into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}
