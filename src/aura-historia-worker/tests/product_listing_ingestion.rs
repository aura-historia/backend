use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_worker::scopes::product_listing_ingestion::{
    ProductListingIngestionJobs, compose_ingestion_use_case,
};
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{
    CustodyReason, Disposition, ErrorClass, GroupKey, JobId, JobQueue, Settlement,
};
use platform_worker::{Job, JobHandler};
use platform_worker_queue_contract::job_queue::product_listing_ingestion_job_scope;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use product_listing_ingestion_queue::{QueueProductListingIngestionPublisher, codec};
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
        CaptureProductListingRawObservationCommand, CreateProductListingCommand,
        IndexedProductListingIngestionIntent, ProductListingIngestionIdempotencyKey,
        ProductListingIngestionIntent, ProductListingIngestionItemOutcome,
        ProductListingIngestionMessage, ProductListingIngestionOutcome,
        ProductListingIngestionSubmission, SubmitInternalProductListingIngestionHandler,
        SubmitInternalProductListingIngestionUseCase, UpdateProductListingCommand,
    },
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

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

/// A first delivery of `body` in the FIFO group its publisher assigned.
fn job(body: &str) -> Job {
    let group = codec::decode(body)
        .and_then(|envelope| codec::fifo_group_id(&envelope))
        .unwrap();
    Job::new(
        JobId::from_uuid(uuid::Uuid::now_v7()),
        1,
        Some(GroupKey::new(group).unwrap()),
        body.to_owned(),
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn real_processor_acknowledges_a_rejected_update_without_blocking_its_group() {
    let pool = get_postgres_client().await;
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party)
        .bind(format!("ingestion-{party}"))
        .bind("FIFO ingestion acceptance test")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')")
        .bind(source.into_uuid())
        .bind(format!("ingestion-{}", source.into_uuid().simple()))
        .bind("FIFO ingestion source")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();

    let create = |key: &str| {
        ProductListingIngestionIntent::Create(CreateProductListingCommand {
            listing_source_id: source,
            source_listing_id: SourceListingId::try_from(key).unwrap(),
            title: None,
            description: None,
            pricing: Default::default(),
            availability: None,
            url: url::Url::parse(&format!("https://example.test/listings/{key}")).unwrap(),
            images: Default::default(),
            auction: None,
        })
    };
    let intents = [
        create("one"),
        ProductListingIngestionIntent::Update {
            product_key: ProductListingKey::new(
                source,
                SourceListingId::try_from("missing").unwrap(),
            ),
            command: UpdateProductListingCommand::default(),
        },
        create("missing"),
        create("three"),
        create("four"),
    ];
    let publisher = CapturingPublisher::default();
    let captured = publisher.0.clone();
    let admission = SubmitInternalProductListingIngestionHandler::new(publisher);
    let result = admission
        .execute(
            &OperationContext {
                principal: Principal::System,
                request_id: RequestId::new("fifo-acceptance-request"),
                correlation_id: CorrelationId::new("fifo-acceptance-correlation"),
            },
            ProductListingIngestionSubmission {
                listing_source_id: source,
                original_input_count: intents.len(),
                idempotency_key: Some(
                    ProductListingIngestionIdempotencyKey::new("fifo-acceptance").unwrap(),
                ),
                items: intents
                    .into_iter()
                    .enumerate()
                    .map(|(index, intent)| IndexedProductListingIngestionIntent { index, intent })
                    .collect(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.confirmed_accepted_count(), 5);
    let bodies: Vec<_> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|message| codec::encode(message).unwrap())
        .collect();
    assert_eq!(bodies.len(), 5);
    let envelopes: Vec<_> = bodies
        .iter()
        .map(|body| codec::decode(body).unwrap())
        .collect();
    assert_eq!(
        codec::fifo_group_id(&envelopes[1]).unwrap(),
        codec::fifo_group_id(&envelopes[2]).unwrap()
    );
    assert_ne!(
        codec::fifo_group_id(&envelopes[1]).unwrap(),
        codec::fifo_group_id(&envelopes[4]).unwrap()
    );
    let command_ids: Vec<_> = envelopes
        .iter()
        .map(|envelope| envelope.command_id().to_owned())
        .collect();

    let jobs = ProductListingIngestionJobs::new(compose_ingestion_use_case(pool.clone()));
    for body in &bodies {
        // The rejected update completes, so it cannot hold the create that follows in its group.
        assert_eq!(Disposition::Complete, jobs.handle(&job(body)).await);
    }

    let listings: Vec<String> = sqlx::query_scalar(
        "SELECT source_listing_id FROM product_listings WHERE listing_source_id = $1 ORDER BY source_listing_id",
    )
    .bind(source.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(listings, ["four", "missing", "one", "three"]);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id IN (SELECT product_listing_id FROM product_listings WHERE listing_source_id = $1)",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 4);
    let mut receipts: Vec<String> = sqlx::query_scalar(
        "SELECT command_id FROM product_listing_command_receipts WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    receipts.sort();
    let mut applied: Vec<_> = [0, 2, 3, 4]
        .into_iter()
        .map(|index| command_ids[index].clone())
        .collect();
    applied.sort();
    // The rejected update leaves no receipt, so a corrected resubmission can still apply.
    assert_eq!(receipts, applied);
}

/// The same retry key, index and listing with `title` as content: a changed title under the
/// same key reuses the command ID with a different semantic fingerprint.
fn reused_key_submission(
    source: ListingSourceId,
    title: &str,
) -> ProductListingIngestionSubmission {
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

async fn seed_queue_source(pool: &PgPool) -> ListingSourceId {
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

/// Queued commands against real PostgreSQL: a command ID reused for different content is
/// admitted as its own job and fails closed at execution instead of applying or replaying.
#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_park_a_command_id_reused_for_different_content_after_the_first_applied() {
    let pool = get_postgres_client().await;
    let source = seed_queue_source(&pool).await;
    let submit = SubmitInternalProductListingIngestionHandler::new(
        QueueProductListingIngestionPublisher::postgres(pool.clone()),
    );
    let context = OperationContext {
        principal: Principal::Service("queue-consumer-test".into()),
        request_id: RequestId::new("queue-consumer-request"),
        correlation_id: CorrelationId::new("queue-consumer-correlation"),
    };
    for title in ["Vase", "Vase, restored"] {
        let result = submit
            .execute(&context, reused_key_submission(source, title))
            .await
            .unwrap();
        assert_eq!(
            ProductListingIngestionOutcome::Accepted,
            result.items[0].outcome
        );
    }
    let queue = JobQueue::new(pool.clone(), product_listing_ingestion_job_scope());
    let jobs = ProductListingIngestionJobs::new(compose_ingestion_use_case(pool.clone()));

    let first = queue.claim(10).await.unwrap().pop().unwrap();
    let lease = *first.lease();
    let applied = jobs.handle(&Job::from(first)).await;
    assert_eq!(Disposition::Complete, applied);
    queue.settle(&lease, applied).await.unwrap();

    let conflict = ErrorClass::new("fingerprint_conflict").unwrap();
    let mut settlements = Vec::new();
    for _ in 0..2 {
        // The released delivery waits out the scope's retry delay; skip it.
        sqlx::query("UPDATE job_queue_jobs SET available_at = now()")
            .execute(&pool)
            .await
            .unwrap();
        let job = queue.claim(10).await.unwrap().pop().unwrap();
        let lease = *job.lease();
        let disposition = jobs.handle(&Job::from(job)).await;
        assert_eq!(Disposition::Park(conflict.clone()), disposition);
        settlements.push(queue.settle(&lease, disposition).await.unwrap());
    }

    assert_eq!(
        vec![
            Settlement::Released,
            Settlement::MovedToCustody(CustodyReason::Parked)
        ],
        settlements,
        "a first delivery is released once; the redelivery is parked"
    );
    let raw_revisions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_revisions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(1, raw_revisions, "the conflicting content never applied");
    let stats = queue.inspect().await.unwrap();
    assert_eq!((0, 1), (stats.depth, stats.in_custody));
}
