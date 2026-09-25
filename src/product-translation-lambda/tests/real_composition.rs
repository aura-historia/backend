use aura_historia_jobs::{
    DomainJob, DomainJobPayload, IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue,
    encode,
};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent, SqsMessage};
use domain_primitives::event_id::EventId;
use lambda_runtime::{Context, LambdaEvent};
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_translation_lambda::{compose_product_translation_use_case_with_model, handler};
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

struct Model {
    calls: Arc<AtomicUsize>,
    fail_first: bool,
    pause: Option<mpsc::Sender<oneshot::Sender<()>>>,
}

#[async_trait::async_trait]
impl LargeLanguageModel for Model {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        assert!(request.prompt.contains("Antiker Eichenstuhl"));
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && call == 0 {
            return Err(LargeLanguageModelError::InvalidResponse {
                source: application::error::box_error(std::io::Error::other(
                    "test provider failure",
                )),
            });
        }
        if let Some(pause) = &self.pause {
            let (resume, wait) = oneshot::channel();
            pause.send(resume).await.expect("provider barrier receiver");
            wait.await.expect("provider barrier released");
        }
        serde_json::from_value(json!({"titles": {
            "en": "Antique oak chair", "fr": "Chaise ancienne en chêne",
            "es": "Silla antigua de roble", "it": "Sedia antica in rovere"
        }}))
        .map_err(|source| LargeLanguageModelError::InvalidResponse {
            source: application::error::box_error(source),
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn provider_retry_commits_once_duplicate_is_noop_and_missing_source_is_retained() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let use_case = compose_product_translation_use_case_with_model(pool.clone(), Model {
            calls: calls.clone(), fail_first: true, pause: None,
        });
        let missing = (ProductListingId::new(), EventId::new());
        assert_eq!(vec!["missing"], failure_ids(handler(event("missing", job_body(missing.0, missing.1)?), use_case.as_ref()).await?));
        assert_eq!(0, calls.load(Ordering::SeqCst));

        let (product, source) = seed_discovery(&pool).await?;
        let body = job_body(product, source)?;
        assert_eq!(vec!["retry"], failure_ids(handler(event("retry", body.clone()), use_case.as_ref()).await?));
        assert!(translations(&pool, product).await?.is_empty());
        assert_eq!(0, completion_count(&pool, product).await?);
        assert!(failure_ids(handler(event("redelivery", body.clone()), use_case.as_ref()).await?).is_empty());
        let committed = translations(&pool, product).await?;
        assert_eq!(vec![
            ("en".to_owned(), "Antique oak chair".to_owned(), *source.as_uuid()),
            ("es".to_owned(), "Silla antigua de roble".to_owned(), *source.as_uuid()),
            ("fr".to_owned(), "Chaise ancienne en chêne".to_owned(), *source.as_uuid()),
            ("it".to_owned(), "Sedia antica in rovere".to_owned(), *source.as_uuid()),
        ], committed);
        assert_eq!(1, completion_count(&pool, product).await?);
        let (current, marker): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
            "SELECT current_event_id, content_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(*source.as_uuid(), marker);
        assert_ne!(*source.as_uuid(), current);
        let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'")
            .bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!(json!({"sourceEventId": source.as_uuid().to_string(), "sourceLanguage": "de", "targetLanguages": ["en", "fr", "es", "it"]}), payload);
        assert!(failure_ids(handler(event("duplicate", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(committed, translations(&pool, product).await?);
        assert_eq!(1, completion_count(&pool, product).await?);
        assert_eq!(3, calls.load(Ordering::SeqCst));
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "translation retry/composition failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn source_change_while_provider_is_in_flight_is_rejected_by_guarded_commit() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (product, source) = seed_discovery(&pool).await?;
        let calls = Arc::new(AtomicUsize::new(0));
        let (pause, mut reached) = mpsc::channel(1);
        let use_case = compose_product_translation_use_case_with_model(pool.clone(), Model {
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
            .bind(json!({"images": {"previousCount": 0, "currentCount": 0}}))
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE product_listings SET content_source_event_id = $1, current_event_id = $1, version = version + 1 WHERE product_listing_id = $2")
            .bind(newer.as_uuid()).bind(product.as_uuid()).execute(&mut *tx).await?;
        tx.commit().await?;
        resume.send(()).map_err(|_| "provider no longer waiting")?;
        assert!(failure_ids(in_flight.await??).is_empty());
        assert!(translations(&pool, product).await?.is_empty());
        assert_eq!(0, completion_count(&pool, product).await?);
        let (current, marker): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
            "SELECT current_event_id, content_source_event_id FROM product_listings WHERE product_listing_id = $1"
        ).bind(product.as_uuid()).fetch_one(&pool).await?;
        assert_eq!((*newer.as_uuid(), *newer.as_uuid()), (current, marker));
        assert!(failure_ids(handler(event("stale-redelivery", body), use_case.as_ref()).await?).is_empty());
        assert_eq!(1, calls.load(Ordering::SeqCst), "stale source should not call provider again");
        assert!(translations(&pool, product).await?.is_empty());
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "translation source-change barrier failed: {result:?}"
    );
}

async fn translations(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<Vec<(String, String, uuid::Uuid)>, sqlx::Error> {
    sqlx::query_as("SELECT language, title, source_event_id FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language")
        .bind(product.as_uuid()).fetch_all(pool).await
}

async fn completion_count(
    pool: &sqlx::PgPool,
    product: ProductListingId,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'")
        .bind(product.as_uuid()).fetch_one(pool).await
}

async fn seed_discovery(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), Box<dyn std::error::Error + Send + Sync>> {
    let product = ProductListingId::new();
    let source = EventId::new();
    let listing_source = ListingSourceId::new();
    let slug = ProductListingSlugId::from_title_and_suffix(
        "translation lambda product",
        &product.as_uuid().simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Translation lambda source', party_id FROM operator")
        .bind(uuid::Uuid::now_v7()).bind(format!("translation-lambda-source-{}", listing_source.as_uuid()))
        .bind(listing_source.as_uuid()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product.as_uuid()).bind(slug.as_ref()).bind(source.as_uuid())
        .bind(listing_source.as_uuid()).bind(product.to_string()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(source.as_uuid()).bind(product.as_uuid()).bind(json!({
            "listingSourceId": listing_source.as_uuid().to_string(), "sourceListingId": product.to_string(),
            "title": {"language": "de", "text": "Antiker Eichenstuhl"}, "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 0, "auction": null
        })).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((product, source))
}

fn job_body(
    product: ProductListingId,
    source: EventId,
) -> Result<String, aura_historia_jobs::WireError> {
    encode(&DomainJob {
        target_queue: WorkerQueue::ProductListingTranslate,
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
        + 50_000;
    LambdaEvent::new(payload, context)
}

fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
    response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect()
}
