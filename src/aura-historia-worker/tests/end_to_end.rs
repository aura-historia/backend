//! The worker process carries one listing end to end against real PostgreSQL and OpenSearch:
//! ingestion command → raw capture → outbox → dispatcher → normalization → ProductListing event
//! → outbox → dispatcher → enrichment workers → OpenSearch projection. Only the external language
//! and embedding models are fakes.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::WorkerScope;
use aura_historia_worker::{
    config::{ScopeConfig, WorkerConfig},
    run,
    scope::ConsumedScope,
    scopes::{
        listing_geography, listing_shipping, product_content_assessment,
        product_embedding::{
            ProductEmbeddingJobs, compose_product_embedding_use_case_with_generator,
        },
        product_listing_ingestion::{ProductListingIngestionJobs, compose_ingestion_use_case},
        product_listing_normalization::{
            ProductListingNormalizationJobs, compose_normalization_use_case,
        },
        product_listing_opensearch::{ProductListingOpenSearchJobs, compose_projection_use_case},
        product_translation::{
            ProductTranslationJobs, compose_product_translation_use_case_with_models,
        },
        watchlist_notification,
    },
};
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use listing_source_core::ListingSourceId;
use opensearch::GetParts;
use platform_postgres_queue::{EnqueueOutcome, GroupKey, NewJob, enqueue};
use platform_worker::{ConsumerConfig, JobHandler};
use platform_worker_queue_contract::job_queue::product_listing_ingestion_job_scope;
use product_listing_ingestion_queue::codec;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_service::{
    ports::{
        ProductListingIngestionPublishError, ProductListingIngestionPublisher,
        ProductListingRawIngestionMethod,
    },
    use_cases::{
        CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
        ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionOutcome, ProductListingIngestionSubmission,
        SubmitInternalProductListingIngestionHandler, SubmitInternalProductListingIngestionUseCase,
    },
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    num::NonZeroU16,
    sync::{Arc, Mutex},
    time::Duration,
};
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client,
};
use tokio_util::sync::CancellationToken;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const POLL: Duration = Duration::from_millis(100);
const DEADLINE: Duration = Duration::from_secs(240);
const SOURCE_RECORD_KEY: &str = "end-to-end-1";

/// Translates every title into every catalogue language.
struct Translator;

#[async_trait::async_trait]
impl LargeLanguageModel for Translator {
    async fn generate<Output>(
        &self,
        _: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        serde_json::from_value(json!({"titles": {
            "de": "Eine antike Keramikvase", "en": "An antique ceramic vase",
            "fr": "Un vase ancien en céramique", "es": "Un jarrón antiguo de cerámica",
            "it": "Un vaso antico in ceramica"
        }}))
        .map_err(|source| LargeLanguageModelError::InvalidResponse {
            source: application::error::box_error(source),
        })
    }
}

struct Embeddings;

#[async_trait::async_trait]
impl EmbeddingGenerator for Embeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        EmbeddingVector::try_new(vec![0.5; EMBEDDING_DIMENSIONS])
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        EmbeddingVector::try_new(vec![0.5; EMBEDDING_DIMENSIONS])
    }
}

/// Keeps the admitted commands instead of sending them, as the ingestion API's publisher would.
#[derive(Clone, Default)]
struct CapturingPublisher(Arc<Mutex<Vec<ProductListingIngestionMessage>>>);

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for CapturingPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let outcomes = commands
            .iter()
            .map(|command| ProductListingIngestionItemOutcome {
                index: command.metadata.index,
                command_id: command.metadata.command_id.clone(),
                outcome: ProductListingIngestionOutcome::Accepted,
            })
            .collect();
        self.0.lock().unwrap().extend(commands);
        Ok(outcomes)
    }
}

async fn seed_listing_source(pool: &PgPool) -> ListingSourceId {
    let party_id = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'End-to-end party')",
    )
    .bind(party_id)
    .bind(format!("end-to-end-{party_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, \
         operator_party_id, listing_source_type) \
         VALUES ($1, $2, 'End-to-end source', $3, 'DEALER')",
    )
    .bind(source.as_uuid())
    .bind(format!("end-to-end-{}", source.as_uuid().simple()))
    .bind(party_id)
    .execute(pool)
    .await
    .unwrap();
    source
}

fn raw_observation(source: ListingSourceId) -> CaptureProductListingRawObservationCommand {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::WoocommerceProduct,
        1,
        1,
        SourcePayload::new(json!({"id": SOURCE_RECORD_KEY})).unwrap(),
        RawProductListingValues::new(json!({
            "sourceListingId": SOURCE_RECORD_KEY,
            "priceFormat": "DISPLAY_TEXT",
            "title": {"action": "SET", "value": "An antique ceramic vase from an English collection"},
            "description": {"action": "SET", "value": ["This antique ceramic vase has documented provenance and careful restoration history."]},
            "price": {"action": "SET", "value": "EUR 420"},
            "priceEstimateMin": {"action": "CLEAR"},
            "priceEstimateMax": {"action": "CLEAR"},
            "availability": {"action": "SET", "value": "in stock"},
            "url": {"action": "SET", "value": "https://example.test/listings/end-to-end-1"},
            "images": {"action": "SET", "value": ["/images/end-to-end-1.jpg"]},
            "attributes": {}
        }))
        .unwrap(),
        NormalizationContext::new(json!({
            "baseUrl": "https://example.test/listings/end-to-end-1",
            "fallbackCurrency": "EUR"
        }))
        .unwrap(),
    )
    .unwrap();
    CaptureProductListingRawObservationCommand {
        listing_source_id: source,
        ingestion_method: ProductListingRawIngestionMethod::Woocommerce,
        source_record_key: SOURCE_RECORD_KEY.to_owned(),
        input,
        provenance: RawProductListingProvenance::new(json!({"deliveryId": SOURCE_RECORD_KEY}))
            .unwrap(),
        source_event_id: Some(SOURCE_RECORD_KEY.to_owned()),
        source_occurred_at: None,
        provider_receipt: None,
    }
}

/// Admits one raw observation as the ingestion API does and enqueues its command into the
/// ingestion FIFO in the group the command carries.
async fn enqueue_ingestion_command(pool: &PgPool, source: ListingSourceId) {
    let publisher = CapturingPublisher::default();
    let captured = Arc::clone(&publisher.0);
    let result = SubmitInternalProductListingIngestionHandler::new(publisher)
        .execute(
            &OperationContext {
                principal: Principal::System,
                request_id: RequestId::new("end-to-end-request"),
                correlation_id: CorrelationId::new("end-to-end-correlation"),
            },
            ProductListingIngestionSubmission {
                listing_source_id: source,
                original_input_count: 1,
                idempotency_key: Some(
                    ProductListingIngestionIdempotencyKey::new("end-to-end").unwrap(),
                ),
                items: vec![IndexedProductListingIngestionIntent {
                    index: 0,
                    intent: ProductListingIngestionIntent::CaptureRaw(raw_observation(source)),
                }],
            },
        )
        .await
        .unwrap();
    assert_eq!(1, result.confirmed_accepted_count());
    let message = captured.lock().unwrap().pop().unwrap();
    let body = codec::encode(&message).unwrap();
    let group = codec::decode(&body)
        .and_then(|envelope| codec::fifo_group_id(&envelope))
        .unwrap();
    let job = NewJob::new(
        &product_listing_ingestion_job_scope(),
        Some(GroupKey::new(group).unwrap()),
        body,
    )
    .unwrap();
    assert!(matches!(
        enqueue(pool, &job).await.unwrap(),
        EnqueueOutcome::Enqueued(_)
    ));
}

async fn handlers(pool: &PgPool) -> Vec<(ConsumedScope, Arc<dyn JobHandler>)> {
    let index = test_api::TestContext::current()
        .expect("active end-to-end test")
        .product_listings_index()
        .expect("scoped product listings index");
    vec![
        (
            ConsumedScope::ProductListingIngestion,
            Arc::new(ProductListingIngestionJobs::new(
                compose_ingestion_use_case(pool.clone()),
            )),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ProductListingRawNormalization),
            Arc::new(ProductListingNormalizationJobs::new(
                compose_normalization_use_case(pool.clone()),
            )),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ProductListingOpenSearch),
            Arc::new(ProductListingOpenSearchJobs::new(
                compose_projection_use_case(pool.clone(), get_opensearch_client().await, index),
            )),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ProductListingTranslation),
            Arc::new(ProductTranslationJobs::new(
                compose_product_translation_use_case_with_models(
                    pool.clone(),
                    Translator,
                    Translator,
                ),
            )),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ProductListingEmbedding),
            Arc::new(ProductEmbeddingJobs::new(
                compose_product_embedding_use_case_with_generator(pool.clone(), Embeddings),
            )),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ProductListingContentAssessment),
            product_content_assessment::compose(pool).await.unwrap(),
        ),
        (
            ConsumedScope::Worker(WorkerScope::WatchlistNotification),
            watchlist_notification::compose(pool).await.unwrap(),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ListingGeography),
            listing_geography::compose(pool).await.unwrap(),
        ),
        (
            ConsumedScope::Worker(WorkerScope::ListingShipping),
            listing_shipping::compose(pool).await.unwrap(),
        ),
    ]
}

fn config(scopes: &[ConsumedScope]) -> WorkerConfig {
    WorkerConfig {
        scopes: scopes
            .iter()
            .map(|scope| ScopeConfig {
                scope: scope.clone(),
                consumer: ConsumerConfig::new(
                    &scope.job_scope(),
                    NonZeroU16::new(2).unwrap(),
                    scope.defaults().job_timeout,
                    POLL,
                    Duration::from_secs(5),
                )
                .unwrap(),
            })
            .collect(),
        dispatch_batch_size: Some(NonZeroU16::new(100).unwrap()),
        poll_interval: POLL,
        shutdown_grace: Duration::from_secs(5),
        health_metrics: None,
    }
}

/// The listing's projected document, once OpenSearch has it.
async fn projected_document(index: &str, product_listing_id: uuid::Uuid) -> Option<Value> {
    let id =
        product_listing_core::product_listing_id::ProductListingId::try_from(product_listing_id)
            .unwrap()
            .to_string();
    let response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id))
        .send()
        .await
        .ok()?;
    let body: Value = response.json().await.ok()?;
    (body["found"] == json!(true)).then_some(body)
}

/// Jobs still waiting or leased in the scopes this worker consumes, outbox rows not yet
/// dispatched, and anything that reached failure custody.
async fn backlog(pool: &PgPool, scopes: &[String]) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM job_queue_jobs WHERE scope = ANY($1)), \
                (SELECT count(*) FROM delivery_outbox), \
                (SELECT count(*) FROM job_queue_failures) \
                  + (SELECT count(*) FROM delivery_outbox_failures)",
    )
    .bind(scopes)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Shortens the retry backoff of released jobs from the scope's delay (minutes) to just past the
/// 60-second work claims the enrichment use cases hold, which a failed attempt leaves in place.
/// Concurrent enrichments of one listing can conflict (SQLSTATE 40001); the loser retries.
async fn fast_forward_retry_backoff(pool: &PgPool) {
    sqlx::query(
        "UPDATE job_queue_jobs \
         SET available_at = GREATEST(now(), last_claimed_at + interval '61 seconds') \
         WHERE lease_token IS NULL AND attempts > 0 \
           AND available_at > GREATEST(now(), last_claimed_at + interval '61 seconds')",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// What remains queued, captured or in custody, by scope and last error class.
async fn stalled_state(pool: &PgPool) -> String {
    let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT 'queued', scope, last_error_class, count(*) FROM job_queue_jobs GROUP BY 2, 3 \
         UNION ALL SELECT 'custody', scope, last_error_class, count(*) FROM job_queue_failures \
           GROUP BY 2, 3 \
         UNION ALL SELECT 'outbox', source_table, NULL, count(*) FROM delivery_outbox GROUP BY 2 \
         UNION ALL SELECT 'outbox_custody', source_table, error_class, count(*) \
           FROM delivery_outbox_failures GROUP BY 2, 3",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    format!("{rows:?}")
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, TestOpenSearch()], isolated = true)]
async fn should_carry_one_ingested_listing_through_every_stage_into_opensearch() {
    let pool = get_postgres_client().await;
    let index = test_api::TestContext::current()
        .unwrap()
        .product_listings_index()
        .unwrap();
    let source = seed_listing_source(&pool).await;
    enqueue_ingestion_command(&pool, source).await;

    let handlers = handlers(&pool).await;
    let scopes: Vec<ConsumedScope> = handlers.iter().map(|(scope, _)| scope.clone()).collect();
    let scope_names: Vec<String> = scopes
        .iter()
        .map(|scope| scope.job_scope().name().as_str().to_owned())
        .collect();
    let shutdown = CancellationToken::new();
    let worker = tokio::spawn(run(
        pool.clone(),
        config(&scopes),
        handlers,
        shutdown.clone(),
    ));

    let reached = tokio::time::timeout(DEADLINE, async {
        loop {
            let listing: Option<uuid::Uuid> = sqlx::query_scalar(
                "SELECT product_listing_id FROM product_listings \
                 WHERE listing_source_id = $1 AND source_listing_id = $2",
            )
            .bind(source.as_uuid())
            .bind(SOURCE_RECORD_KEY)
            .fetch_optional(&pool)
            .await
            .unwrap();
            if let Some(listing) = listing
                && let Some(document) = projected_document(&index, listing).await
                && backlog(&pool, &scope_names).await == (0, 0, 0)
            {
                return (listing, document);
            }
            fast_forward_retry_backoff(&pool).await;
            tokio::time::sleep(POLL).await;
        }
    })
    .await;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(30), worker)
        .await
        .expect("the worker drains and stops after shutdown")
        .unwrap()
        .expect("no worker task stopped before shutdown");

    let Ok((listing, document)) = reached else {
        panic!(
            "the listing did not reach OpenSearch with every queue drained in time: {}",
            stalled_state(&pool).await
        )
    };
    assert_ne!(json!(true), document["_source"]["projectionDeleted"]);
    let current_event: uuid::Uuid = sqlx::query_scalar(
        "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(listing)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        json!(
            domain_primitives::event_id::EventId::try_from(current_event)
                .unwrap()
                .to_string()
        ),
        document["_source"]["eventId"],
        "the projection holds the listing's current event"
    );
    let translations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_translations WHERE product_listing_id = $1",
    )
    .bind(listing)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(translations > 0, "the translation worker ran");
    let embedded: bool = sqlx::query_scalar(
        "SELECT embedding IS NOT NULL FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(listing)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(embedded, "the embedding worker ran");
}
