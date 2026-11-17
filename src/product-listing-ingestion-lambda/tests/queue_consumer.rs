//! Queued ingestion commands against real PostgreSQL: a command ID reused for different content
//! is admitted as its own job and fails closed at execution instead of applying or replaying.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{CustodyReason, Disposition, ErrorClass, JobQueue, Settlement};
use platform_worker_queue_contract::job_queue::product_listing_ingestion_job_scope;
use product_listing_ingestion_lambda::{compose_ingestion_use_case, process_queued_ingestion_job};
use product_listing_ingestion_queue::QueueProductListingIngestionPublisher;
use product_listing_normalization::{
    NormalizationContext, ProductListingNormalizationInput, RawProductListingOperation,
    RawProductListingPayloadFormat, RawProductListingProvenance, RawProductListingValues,
    SourcePayload,
};
use product_listing_service::{
    ports::ProductListingRawIngestionMethod,
    use_cases::{
        CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
        ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
        ProductListingIngestionOutcome, ProductListingIngestionSubmission,
        SubmitInternalProductListingIngestionHandler, SubmitInternalProductListingIngestionUseCase,
    },
};
use serde_json::json;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

async fn seed_source(pool: &PgPool) -> ListingSourceId {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party)
        .bind(format!("queue-{party}"))
        .bind("Queue consumer test")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, \
         operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')",
    )
    .bind(source.into_uuid())
    .bind(format!("queue-{}", source.into_uuid().simple()))
    .bind("Queue consumer source")
    .bind(party)
    .execute(pool)
    .await
    .unwrap();
    source
}

/// The same retry key, index and listing with `title` as content: a changed title under the
/// same key reuses the command ID with a different semantic fingerprint.
fn submission(source: ListingSourceId, title: &str) -> ProductListingIngestionSubmission {
    let input = ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        SourcePayload::new(json!({ "title": title })).unwrap(),
        RawProductListingValues::new(json!({})).unwrap(),
        NormalizationContext::new(json!({})).unwrap(),
    )
    .unwrap();
    ProductListingIngestionSubmission {
        listing_source_id: source,
        original_input_count: 1,
        idempotency_key: Some(ProductListingIngestionIdempotencyKey::new("reused-key").unwrap()),
        items: vec![IndexedProductListingIngestionIntent {
            index: 0,
            intent: ProductListingIngestionIntent::CaptureRaw(
                CaptureProductListingRawObservationCommand {
                    listing_source_id: source,
                    ingestion_method: ProductListingRawIngestionMethod::WebCrawl,
                    source_record_key: "https://shop.example/vase".into(),
                    input,
                    provenance: RawProductListingProvenance::new(json!({})).unwrap(),
                    source_event_id: None,
                    source_occurred_at: None,
                    provider_receipt: None,
                },
            ),
        }],
    }
}

async fn admit(
    submit: &SubmitInternalProductListingIngestionHandler<QueueProductListingIngestionPublisher>,
    submission: ProductListingIngestionSubmission,
) {
    let context = OperationContext {
        principal: Principal::Service("queue-consumer-test".into()),
        request_id: RequestId::new("queue-consumer-request"),
        correlation_id: CorrelationId::new("queue-consumer-correlation"),
    };
    let result = submit.execute(&context, submission).await.unwrap();
    assert_eq!(
        ProductListingIngestionOutcome::Accepted,
        result.items[0].outcome
    );
}

async fn raw_revisions(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_revisions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_park_a_command_id_reused_for_different_content_after_the_first_applied() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let submit = SubmitInternalProductListingIngestionHandler::new(
        QueueProductListingIngestionPublisher::postgres(pool.clone()),
    );
    admit(&submit, submission(source, "Vase")).await;
    admit(&submit, submission(source, "Vase, restored")).await;
    let queue = JobQueue::new(pool.clone(), product_listing_ingestion_job_scope());
    let use_case = compose_ingestion_use_case(pool.clone());

    let first = queue.claim(10).await.unwrap().pop().unwrap();
    let applied =
        process_queued_ingestion_job(first.job_id(), first.payload(), use_case.as_ref()).await;
    assert_eq!(Disposition::Complete, applied);
    queue.settle(first.lease(), applied).await.unwrap();

    let conflict = ErrorClass::new("fingerprint_conflict").unwrap();
    let mut settlements = Vec::new();
    for _ in 0..2 {
        // The released delivery waits out the scope's retry delay; skip it.
        sqlx::query("UPDATE job_queue_jobs SET available_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        let job = queue.claim(10).await.unwrap().pop().unwrap();
        let disposition =
            process_queued_ingestion_job(job.job_id(), job.payload(), use_case.as_ref()).await;
        assert_eq!(Disposition::Park(conflict.clone()), disposition);
        settlements.push(queue.settle(job.lease(), disposition).await.unwrap());
    }

    assert_eq!(
        vec![
            Settlement::Released,
            Settlement::MovedToCustody(CustodyReason::Parked)
        ],
        settlements,
        "a first delivery is released once; the redelivery is parked"
    );
    assert_eq!(
        1,
        raw_revisions(&pool).await,
        "the conflicting content never applied"
    );
    let stats = queue.inspect().await.unwrap();
    assert_eq!((0, 1), (stats.depth, stats.in_custody));
}
