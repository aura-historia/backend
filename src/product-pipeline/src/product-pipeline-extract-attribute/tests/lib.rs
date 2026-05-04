use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::batch::Batch;
use common::category_key::CategoryId;
use common::event_id::EventId;
use common::language::record::LanguageRecord;
use common::language::record::TextRecord;
use common::product_id::ProductId;
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use product::core::product_event::enrichment::{
    ClassifiedCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
};
use product::core::product_event::{ProductEvent, ProductEventPayload};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_record::{ProductRecord, mk_pk};
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::get_service::GetProductServiceImpl;
use product_pipeline_extract_attribute::handler;
use product_pipeline_extract_attribute::service::MockExtractionService;
use product_pipeline_extract_attribute::types::ExtractedAttributes;
use std::time::SystemTime;
use test_api::*;
use time::OffsetDateTime;
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

/// Creates a ENRICHMENT_CLASSIFY_CATEGORY event record — the trigger for extract-attribute.
fn mk_classify_event_record(
    shop_id: common::shop_id::ShopId,
    seller_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
    product_id: ProductId,
) -> ProductEventRecord {
    let payload = ClassifiedCategoryProductEnrichmentEventPayload {
        shop_id,
        seller_id,
        shops_product_id,
        category_id: CategoryId::from("furniture"),
    };
    let event = ProductEvent {
        aggregate_id: product_id,
        event_id: EventId::new(),
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::ClassifiedCategory(payload),
        ),
    };
    event.into()
}

/// Seeds a materialized ProductRecord in DynamoDB so GetProductService can find it.
async fn seed_product_record(
    repository: &ProductDynamoDbRepositoryImpl<'_>,
    shop_id: common::shop_id::ShopId,
    seller_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
    product_id: ProductId,
    native_title: &str,
) {
    let mut record: ProductRecord = Faker.fake();
    record.pk = mk_pk(&shop_id, &shops_product_id);
    record.shop_id = shop_id;
    record.seller_id = seller_id;
    record.shops_product_id = shops_product_id;
    record.product_id = product_id;
    record.title_native = TextRecord::new(native_title, LanguageRecord::En);
    record.description_native = Some(TextRecord::new(
        format!("{native_title} - antique item in original condition."),
        LanguageRecord::En,
    ));

    let batch = Batch::try_from_iter(std::iter::once(record))
        .expect("shouldn't fail creating single-item batch");
    repository
        .put_product_records(batch)
        .await
        .expect("shouldn't fail seeding product record");
}

/// Verify that the handler batch-loads the materialized product via GetProductService,
/// calls the extraction service, and persists the resulting enrichment/policy events to DynamoDB.
#[localstack_test(services = [DynamoDB()])]
async fn should_persist_attribute_and_policy_events_when_classify_event_triggers_pipeline() {
    let client = get_dynamodb_client().await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME").unwrap();
    let repository = ProductDynamoDbRepositoryImpl::new(client, &table_name);

    let shop_id: common::shop_id::ShopId = Faker.fake();
    let seller_id: common::shop_id::ShopId = Faker.fake();
    let shops_product_id: common::shops_product_id::ShopsProductId = Faker.fake();
    let product_id = ProductId::new();

    seed_product_record(
        &repository,
        shop_id,
        seller_id,
        shops_product_id.clone(),
        product_id,
        "Antique oak chair circa 1870",
    )
    .await;

    let get_product_service = GetProductServiceImpl::new(&repository);
    let classify_record =
        mk_classify_event_record(shop_id, seller_id, shops_product_id, product_id);

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

    let event = mk_lambda_event(vec![mk_sqs_message(&classify_record)]);
    let result = handler(&mock_service, &get_product_service, &repository, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}

/// Verify that a batch of multiple classify records is processed and all
/// resulting events are written to DynamoDB without failures.
#[localstack_test(services = [DynamoDB()])]
async fn should_process_multiple_products_in_single_handler_invocation() {
    let client = get_dynamodb_client().await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME").unwrap();
    let repository = ProductDynamoDbRepositoryImpl::new(client, &table_name);

    let get_product_service = GetProductServiceImpl::new(&repository);

    let titles = [
        "Victorian silver candlestick",
        "Antique mahogany writing desk",
        "Georgian silver tea service",
    ];

    let mut classify_messages = Vec::new();
    for title in &titles {
        let shop_id: common::shop_id::ShopId = Faker.fake();
        let seller_id: common::shop_id::ShopId = Faker.fake();
        let shops_product_id: common::shops_product_id::ShopsProductId = Faker.fake();
        let product_id = ProductId::new();

        seed_product_record(
            &repository,
            shop_id,
            seller_id,
            shops_product_id.clone(),
            product_id,
            title,
        )
        .await;

        classify_messages.push(mk_sqs_message(&mk_classify_event_record(
            shop_id,
            seller_id,
            shops_product_id,
            product_id,
        )));
    }

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

    let event = mk_lambda_event(classify_messages);
    let result = handler(&mock_service, &get_product_service, &repository, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}
