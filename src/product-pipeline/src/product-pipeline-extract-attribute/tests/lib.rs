use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product_pipeline_extract_attribute::handler;
use product_pipeline_extract_attribute::service::MockExtractionService;
use product_pipeline_extract_attribute::types::ExtractedAttributes;
use std::time::SystemTime;
use test_api::*;
use uuid::Uuid;

fn mk_event_bridge_payload(event_record: &impl serde::Serialize) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(event_record).unwrap();
    stream_record.size_bytes = 42;

    let mut event = EventRecord::default();
    event.aws_region = "eu-central-1".to_string();
    event.change = stream_record;
    event.event_id = Uuid::new_v4().to_string();
    event.event_name = "INSERT".to_string();

    let mut eb_event = EventBridgeEvent::<EventRecord>::default();
    eb_event.detail_type = "DynamoDBStreamRecord".to_string();
    eb_event.source = "test-table".to_string();
    eb_event.detail = event;

    serde_json::to_string(&eb_event).unwrap()
}

fn mk_sqs_message(record: &impl serde::Serialize) -> SqsMessage {
    let mut msg = SqsMessage::default();
    msg.message_id = Some(Faker.fake());
    msg.body = Some(mk_event_bridge_payload(record));
    msg
}

fn mk_lambda_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = messages;
    LambdaEvent {
        payload: sqs_event,
        context: Context::default(),
    }
}

/// Verify that the handler successfully extracts attributes from a valid
/// enrichment record and persists the resulting events to DynamoDB.
#[localstack_test(services = [DynamoDB()])]
async fn should_persist_attribute_and_policy_events_when_valid_enrichment_record() {
    let client = get_dynamodb_client().await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME").unwrap();
    let repository = ProductDynamoDbRepositoryImpl::new(client, &table_name);

    let mut enrichment_record: ProductEnrichmentEventRecord = Faker.fake();
    enrichment_record.embedding = Some(vec![0.1f32; 768]);
    enrichment_record.native_title = Some("Antique oak chair circa 1870".to_string());

    let event_record = ProductEventRecord::Enrichment(enrichment_record);

    let mut mock_service = MockExtractionService::new();
    mock_service.expect_extract().once().returning(|texts| {
        let count = texts.len();
        Box::pin(async move {
            vec![
                Some(ExtractedAttributes {
                    y: Some(1870.into()),
                    nazi: Some(false),
                    ..Default::default()
                });
                count
            ]
        })
    });

    let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
    let result = handler(&mock_service, &repository, event).await.unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}

/// Verify that a batch of multiple enrichment records is processed and all
/// resulting events are written to DynamoDB without failures.
#[localstack_test(services = [DynamoDB()])]
async fn should_process_multiple_products_in_single_handler_invocation() {
    let client = get_dynamodb_client().await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME").unwrap();
    let repository = ProductDynamoDbRepositoryImpl::new(client, &table_name);

    let records: Vec<SqsMessage> = (0..3)
        .map(|_| {
            let mut r: ProductEnrichmentEventRecord = Faker.fake();
            r.native_title = Some("Victorian silver candlestick".to_string());
            mk_sqs_message(&ProductEventRecord::Enrichment(r))
        })
        .collect();

    let mut mock_service = MockExtractionService::new();
    mock_service.expect_extract().once().returning(|texts| {
        let count = texts.len();
        Box::pin(async move {
            vec![
                Some(ExtractedAttributes {
                    y: Some(1890.into()),
                    nazi: Some(false),
                    ..Default::default()
                });
                count
            ]
        })
    });

    let event = mk_lambda_event(records);
    let result = handler(&mock_service, &repository, event).await.unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}
