use aura_historia_worker::{
    QueueConfig, WorkerRunError, WorkerRuntimeComposition, WorkerScope,
    product_content_assessment::consume_product_content_assessment_queue, serve_with_runtime,
};
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSourceReader,
    SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::use_cases::{
    AssessProductListingContentEventHandler, AssessProductListingContentEventUseCase,
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_assess_committed_created_product_event_as_allowed() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, content_source_event_id) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Antiker Eichenstuhl",
            "Bemalter Stuhl",
        )
        .await?;

        let assessment = wait_for_assessment(&worker.pool, product_listing_id).await?;

        assert_eq!(uuid::Uuid::from(content_source_event_id), assessment.0);
        assert_eq!("ALLOWED", assessment.1);
        assert_eq!(None, assessment.2);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_assess_committed_created_product_event_as_requires_consent() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, content_source_event_id) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "Historisches Abzeichen.",
        )
        .await?;

        let assessment = wait_for_assessment(&worker.pool, product_listing_id).await?;

        assert_eq!(uuid::Uuid::from(content_source_event_id), assessment.0);
        assert_eq!("REQUIRES_CONSENT", assessment.1);
        assert_eq!(Some("NAZI_GERMANY".to_owned()), assessment.2);
        Ok(())
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_SEQUIN])]
async fn should_not_assess_committed_price_event() {
    let worker = ContentAssessmentWorker::start().await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let (product_listing_id, _) = insert_product_with_event(
            &worker.pool,
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            "Hakenkreuz-Abzeichen",
            "This event must not route to content assessment.",
        )
        .await?;

        assert_no_assessment(&worker.pool, product_listing_id, NO_SIDE_EFFECT_OBSERVATION).await
    }
    .await;
    worker
        .finish(result)
        .await
        .unwrap_or_else(|error| panic!("worker cleanup or test failed: {error}"));
}

struct ContentAssessmentWorker {
    pool: sqlx::PgPool,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    consumer: JoinHandle<()>,
}

impl ContentAssessmentWorker {
    async fn start() -> Self {
        let pool = get_postgres_client().await;
        let handler: Arc<dyn AssessProductListingContentEventUseCase> =
            Arc::new(AssessProductListingContentEventHandler::new(
                SqlxProductListingContentAssessmentSourceReader::new(pool.clone()),
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingContentAssessmentWriterFactory::new(),
            ));
        let composition = WorkerRuntimeComposition::build(
            WorkerScope::ProductListingContentAssessment,
            QueueConfig::new(16),
        )
        .unwrap_or_else(|error| {
            panic!("valid product-content-assessment queue configuration: {error}")
        });
        let (runtime, receiver) = composition.into_parts();
        let consumer = tokio::spawn(consume_product_content_assessment_queue(receiver, handler));
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

async fn insert_product_with_event(
    pool: &sqlx::PgPool,
    event_type: &str,
    event_group: &str,
    title: &str,
    description: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let event_id = EventId::new();
    let title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "content assessment worker product",
        &uuid::Uuid::from(product_listing_id).simple().to_string()[..6],
    )
    .map_err(|_| sqlx::Error::Protocol("invalid fixture title slug".to_owned()))?;
    let listing_source_id = uuid::Uuid::new_v4();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, 'Content assessment worker source', party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("content-assessment-worker-source-{listing_source_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $4, $5, $6, 'de', $7, 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(title_slug_id.as_ref())
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id)
        .bind(product_listing_id.to_string())
        .bind(title)
        .bind(description)

        .execute(&mut *tx)
        .await?;
    let payload = serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": product_listing_id.to_string(),
        "title": {"language": "de", "text": title},
        "description": {"language": "de", "text": description},
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE",
        "url": "https://example.test/product",
        "imageCount": 0,
        "auction": {"start": null, "end": null}
    });
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

async fn wait_for_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(uuid::Uuid, String, Option<String>), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(assessment) = assessment(pool, product_listing_id).await? {
            return Ok(assessment);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other("timed out waiting for product content assessment").into())
}

async fn assert_no_assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        if assessment(pool, product_listing_id).await?.is_some() {
            return Err(
                std::io::Error::other("unexpected product content assessment persisted").into(),
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Ok(())
}

async fn assessment(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<Option<(uuid::Uuid, String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT source_event_id, decision, category FROM product_listing_content_assessments WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_optional(pool)
    .await
}
