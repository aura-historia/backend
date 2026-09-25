use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_embedding_lambda::{compose_product_embedding_use_case_with_generator, handler};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::{mpsc, oneshot};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct Generator {
    calls: Arc<AtomicUsize>,
    fail_first: bool,
    pause: Option<mpsc::Sender<oneshot::Sender<()>>>,
}

#[async_trait::async_trait]
impl EmbeddingGenerator for Generator {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        description: Option<&EmbeddingText>,
        image: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        assert_eq!("Antiker Eichenstuhl", title.as_str());
        assert_eq!(
            Some("Bemalter Stuhl"),
            description.map(EmbeddingText::as_str)
        );
        assert_eq!(
            Some("https://example.test/image.jpg"),
            image.map(|url| url.as_url().as_str())
        );
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && call == 0 {
            return Err(EmbeddingError::InvalidResponse {
                reason: "test provider failure",
            });
        }
        if let Some(pause) = &self.pause {
            let (resume, wait) = oneshot::channel();
            pause.send(resume).await.expect("provider barrier receiver");
            wait.await.expect("provider barrier released");
        }
        EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("unexpected query embedding")
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn provider_retry_commits_once_duplicate_is_noop_and_missing_source_is_retained() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let use_case = compose_product_embedding_use_case_with_generator(pool.clone(), Generator {
            calls: calls.clone(), fail_first: true, pause: None,
        });
        let missing = (ProductListingId::new(), EventId::new());
        assert_eq!(vec!["missing"], failure_ids(handler(event("missing", job_body(missing.0, missing.1)?), use_case.as_ref()).await?));
        assert_eq!(0, calls.load(Ordering::SeqCst));

        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        assert_eq!(vec!["retry"], failure_ids(handler(event("retry", body.clone()), use_case.as_ref()).await?));
        assert_eq!(0, completion_count(&pool, product).await?);
        assert!(embedding(&pool, product).await?.0.is_none());
        assert!(failure_ids(handler(event("redelivery", body.clone()), use_case.as_ref()).await?).is_empty());
        let committed = embedding(&pool, product).await?;
        let vector = committed.0.as_ref().ok_or("missing committed embedding")?;
        assert_eq!(EMBEDDING_DIMENSIONS, vector.len());
        assert!((vector[0] - 1.0 / (EMBEDDING_DIMENSIONS as f32).sqrt()).abs() < 0.000_001);
        assert_eq!(*source.as_uuid(), committed.2);
        assert_ne!(*source.as_uuid(), committed.1);
        assert_eq!(1, completion_count(&pool, product).await?);
        let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(json!({"sourceEventId": source.as_uuid().to_string()}), payload);
        assert!(failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(committed, embedding(&pool, product).await?);
        assert_eq!(1, completion_count(&pool, product).await?);
        assert_eq!(3, calls.load(Ordering::SeqCst));
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "embedding retry/composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn source_change_while_provider_is_in_flight_is_rejected_by_guarded_commit() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (product, source) = seed_discovery(&pool).await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let (pause, mut reached) = mpsc::channel(1);
        let use_case = compose_product_embedding_use_case_with_generator(pool.clone(), Generator {
            calls: calls.clone(), fail_first: false, pause: Some(pause),
        });
        let body = job_body(product, source)?;
        let in_flight = tokio::spawn({
            let use_case = use_case.clone();
            let body = body.clone();
            async move { handler(event("in-flight", body), use_case.as_ref()).await }
        });
        let resume = tokio::time::timeout(Duration::from_secs(10), reached.recv()).await?
            .ok_or("provider not reached")?;
        let newer = EventId::new();
        let mut tx = pool.begin().await?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())")
            .bind(newer.as_uuid()).bind(product.as_uuid())
            .bind(json!({"images": {"previousCount": 1, "currentCount": 1}}))
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE product_listings SET embedding_source_event_id = $1, current_event_id = $1, embedding = NULL, version = version + 1 WHERE product_listing_id = $2")
            .bind(newer.as_uuid()).bind(product.as_uuid()).execute(&mut *tx).await?;
        tx.commit().await?;
        resume.send(()).map_err(|_| "provider no longer waiting")?;
        assert!(failure_ids(in_flight.await??).is_empty());
        assert_eq!(0, completion_count(&pool, product).await?);
        let stored = embedding(&pool, product).await?;
        assert!(stored.0.is_none());
        assert_eq!(*newer.as_uuid(), stored.1);
        assert_eq!(*newer.as_uuid(), stored.2);
        assert!(failure_ids(handler(event("stale-redelivery", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(1, calls.load(Ordering::SeqCst), "stale source should not call provider again");
        assert_eq!(0, completion_count(&pool, product).await?);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "embedding source-change barrier failed: {result:?}"
    );
}

async fn embedding(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<(Option<Vec<f32>>, uuid::Uuid, uuid::Uuid), sqlx::Error> {
    sqlx::query_as("SELECT embedding, current_event_id, embedding_source_event_id FROM product_listings WHERE product_listing_id = $1")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn completion_count(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_EMBEDDED'")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn seed_discovery(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), Box<dyn std::error::Error + Send + Sync>> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let slug = ProductListingSlugId::from_title_and_suffix(
        "embedding lambda product",
        &product.as_uuid().simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Embedding lambda source', party_id FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("embedding-lambda-source-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[{\"url\": \"https://example.test/image.jpg\"}]')")
        .bind(product.as_uuid()).bind(slug.as_ref()).bind(source.as_uuid())
        .bind(listing_source.as_uuid()).bind(product.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid()).bind(json!({
            "listingSourceId": listing_source.as_uuid().to_string(), "sourceListingId": product.to_string(),
            "title": {"language": "de", "text": "Antiker Eichenstuhl"},
            "description": {"language": "de", "text": "Bemalter Stuhl"},
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 1, "auction": null
        })).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingEmbed,
        idempotency_key: IdempotencyKey::new(format!("product-event:{source}")),
        ordering_key: OrderingKey::new(format!("product:{product}")),
        payload: DomainJobPayload::<aura_historia_jobs::SearchFilterOperation>::ProductListingEvent(
            ProductListingEventJob {
                event_id: source,
                product_listing_id: product,
            },
        ),
    })
}

fn event(id: &str, body: String) -> LambdaEvent<SqsEvent> {
    let mut message = SqsMessage::default();
    message.message_id = Some(id.to_owned());
    message.body = Some(body);
    let mut payload = SqsEvent::default();
    payload.records = vec![message];
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 65_000;
    LambdaEvent::new(payload, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}
