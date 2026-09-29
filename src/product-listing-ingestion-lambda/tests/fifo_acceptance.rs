use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use product_listing_ingestion_lambda::{compose_ingestion_use_case, handler};
use product_listing_ingestion_sqs::codec;
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        CreateProductListingCommand, IndexedProductListingIngestionIntent,
        ProcessProductListingIngestionUseCase, ProductListingIngestionCompletion,
        ProductListingIngestionEnvelope, ProductListingIngestionError,
        ProductListingIngestionIdempotencyKey, ProductListingIngestionIntent,
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionOutcome, ProductListingIngestionSubmission,
        SubmitInternalProductListingIngestionHandler, SubmitInternalProductListingIngestionUseCase,
        UpdateProductListingCommand,
    },
};
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct RecordingProcessor {
    seen: Mutex<Vec<String>>,
    fail_command: Option<String>,
}

#[async_trait::async_trait]
impl ProcessProductListingIngestionUseCase for RecordingProcessor {
    async fn execute(
        &self,
        command: ProductListingIngestionEnvelope,
    ) -> Result<ProductListingIngestionCompletion, ProductListingIngestionError> {
        let id = command.message.metadata.command_id.as_str().to_owned();
        self.seen.lock().unwrap().push(id.clone());
        if self.fail_command.as_ref() == Some(&id) {
            Err(ProductListingIngestionError::BeginTransactionFailed)
        } else {
            Ok(ProductListingIngestionCompletion::AlreadyCompleted)
        }
    }
}

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

fn fixture_bodies() -> Vec<String> {
    let values: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../product-listing-ingestion-sqs/tests/fixtures/ingestion_v1.json"
    ))
    .unwrap();
    values.into_iter().map(|value| value.to_string()).collect()
}

fn event(bodies: &[String]) -> LambdaEvent<SqsEvent> {
    let mut payload = SqsEvent::default();
    payload.records = bodies
        .iter()
        .enumerate()
        .map(|(index, body)| {
            let mut record = SqsMessage::default();
            record.message_id = Some(format!("sqs-{index}"));
            record.body = Some(body.clone());
            record
        })
        .collect();
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 45_000;
    LambdaEvent::new(payload, context)
}

#[tokio::test]
async fn confirmed_prefix_is_not_retried_and_failed_suffix_never_starts() {
    let bodies = fixture_bodies();
    let command_ids: Vec<_> = bodies
        .iter()
        .map(|body| codec::decode(body).unwrap().command_id().to_owned())
        .collect();
    // The unprocessed suffix includes a different FIFO group: a batch-level stop
    // must not start its work just because its group is independent.
    assert_ne!(
        codec::fifo_group_id(&codec::decode(&bodies[0]).unwrap()).unwrap(),
        codec::fifo_group_id(&codec::decode(&bodies[4]).unwrap()).unwrap()
    );
    let ten: Vec<_> = (0..10)
        .map(|index| bodies[index % bodies.len()].clone())
        .collect();
    let processor = RecordingProcessor {
        seen: Mutex::new(Vec::new()),
        fail_command: Some(command_ids[1].clone()),
    };
    let response = handler(event(&ten), &processor).await.unwrap();
    let failures: Vec<_> = response
        .batch_item_failures
        .into_iter()
        .map(|failure| failure.item_identifier)
        .collect();
    assert_eq!(
        failures,
        (1..10)
            .map(|index| format!("sqs-{index}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(*processor.seen.lock().unwrap(), command_ids[..2]);

    // Simulate redelivery of the suffix only. A post-hoc failure filter over a batch
    // which had already run records 2-9 would fail the assertion above.
    let replay = RecordingProcessor {
        seen: Mutex::new(Vec::new()),
        fail_command: None,
    };
    let response = handler(event(&ten[1..]), &replay).await.unwrap();
    assert!(response.batch_item_failures.is_empty());
    assert_eq!(replay.seen.lock().unwrap().len(), 9);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn real_processor_commits_prefix_and_never_executes_failed_fifo_suffix() {
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
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
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

    let processor = compose_ingestion_use_case(pool.clone());
    let response = handler(event(&bodies), processor.as_ref()).await.unwrap();
    assert_eq!(
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>(),
        ["sqs-1", "sqs-2", "sqs-3", "sqs-4"]
    );

    let listings: Vec<String> = sqlx::query_scalar(
        "SELECT source_listing_id FROM product_listings WHERE listing_source_id = $1 ORDER BY source_listing_id",
    )
    .bind(source.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(listings, ["one"]);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id IN (SELECT product_listing_id FROM product_listings WHERE listing_source_id = $1)",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);
    let receipts: Vec<String> = sqlx::query_scalar(
        "SELECT command_id FROM product_listing_command_receipts WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, command_ids[..1]);
}

#[tokio::test]
async fn empty_single_and_malformed_first_record_have_exact_failure_boundaries() {
    let bodies = fixture_bodies();
    let processor = RecordingProcessor {
        seen: Mutex::new(Vec::new()),
        fail_command: None,
    };
    assert!(
        handler(event(&[]), &processor)
            .await
            .unwrap()
            .batch_item_failures
            .is_empty()
    );
    assert!(
        handler(event(&bodies[..1]), &processor)
            .await
            .unwrap()
            .batch_item_failures
            .is_empty()
    );
    assert_eq!(processor.seen.lock().unwrap().len(), 1);
    let response = handler(
        event(&["not-json".to_owned(), bodies[0].clone()]),
        &processor,
    )
    .await
    .unwrap();
    assert_eq!(
        response
            .batch_item_failures
            .into_iter()
            .map(|f| f.item_identifier)
            .collect::<Vec<_>>(),
        ["sqs-0", "sqs-1"]
    );
    assert_eq!(processor.seen.lock().unwrap().len(), 1);
}
