use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aura_historia_worker::scopes::product_embedding::{
    ProductEmbeddingJobs, compose_product_embedding_use_case_with_generator,
};
use domain_primitives::event_id::EventId;
use embedding::{
    EMBEDDING_DIMENSIONS, EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText,
    EmbeddingVector,
};
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{Disposition, JobId};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
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

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn provider_retry_commits_once_duplicate_is_noop_and_missing_source_is_retained() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(ProductEmbeddingJobs::new(compose_product_embedding_use_case_with_generator(pool.clone(), Generator {
            calls: calls.clone(), fail_first: true, pause: None,
        })));
        let missing = (ProductListingId::new(), EventId::new());
        assert_eq!(Disposition::Retry(error_class("missing_source")), handler.handle(&job(&job_body(missing.0, missing.1)?)).await);
        assert_eq!(0, calls.load(Ordering::SeqCst));

        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        assert_eq!(Disposition::Retry(error_class("embedding_unavailable")), handler.handle(&job(&body)).await);
        assert_eq!(0, completion_count(&pool, product).await?);
        assert!(embedding(&pool, product).await?.0.is_none());
        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
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
        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
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

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn source_change_while_provider_is_in_flight_is_rejected_by_guarded_commit() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (product, source) = seed_discovery(&pool).await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let (pause, mut reached) = mpsc::channel(1);
        let handler = Arc::new(ProductEmbeddingJobs::new(compose_product_embedding_use_case_with_generator(pool.clone(), Generator {
            calls: calls.clone(), fail_first: false, pause: Some(pause),
        })));
        let body = job_body(product, source)?;
        let in_flight = test_api::spawn_owned({
            let handler = handler.clone();
            let body = body.clone();
            async move { handler.handle(&job(&body)).await }
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
        assert_eq!(Disposition::Complete, in_flight.await?);
        assert_eq!(0, completion_count(&pool, product).await?);
        let stored = embedding(&pool, product).await?;
        assert!(stored.0.is_none());
        assert_eq!(*newer.as_uuid(), stored.1);
        assert_eq!(*newer.as_uuid(), stored.2);
        assert_eq!(Disposition::Complete, handler.handle(&job(&body)).await);
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
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) SELECT $3, $2, 'Embedding lambda source', party_id, 'DEALER' FROM operator")
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
        payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
            event_id: source,
            product_listing_id: product,
        }),
    })
}

fn job(body: &str) -> Job {
    Job::new(
        JobId::from_uuid(uuid::Uuid::now_v7()),
        1,
        None,
        body.to_owned(),
    )
}
