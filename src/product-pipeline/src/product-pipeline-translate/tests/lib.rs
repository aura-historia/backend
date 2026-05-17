use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::batch::Batch;
use common::event::Event;
use common::event_id::EventId;
use common::language::domain::Language;
use common::language::record::{LanguageRecord, TextRecord};
use common::product_id::ProductId;
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use product::core::product_event::domain::{
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_record::{ProductRecord, mk_pk};
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::dynamodb::test_utils::ProductRecordSeedExt;
use product::service::get_service::GetProductServiceImpl;
use product_pipeline_translate::handler;
use product_pipeline_translate::service::MockTranslationService;
use std::collections::HashMap;
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

/// Creates a DOMAIN_CREATED event record — the trigger for translate.
fn mk_domain_created_event_record(
    shop_id: common::shop_id::ShopId,
    seller_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
    product_id: ProductId,
) -> ProductEventRecord {
    let mut payload: ProductCreatedDomainEventPayload = Faker.fake();
    payload.shop_id = shop_id;
    payload.seller_id = seller_id;
    payload.shops_product_id = shops_product_id;
    let event = Event {
        aggregate_id: product_id,
        event_id: EventId::new(),
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductDomainEventPayload::Created(payload),
    };
    let domain_event: product::dynamodb::product_event_record::domain::ProductDomainEventRecord =
        event.into();
    ProductEventRecord::Domain(domain_event)
}

async fn seed_product_record(
    repository: &ProductDynamoDbRepositoryImpl<'_>,
    shop_id: common::shop_id::ShopId,
    seller_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
    product_id: ProductId,
    native_title: &str,
    native_language: LanguageRecord,
) {
    let mut record: ProductRecord = Faker.fake();
    record.pk = mk_pk(&shop_id, &shops_product_id);
    record.shop_id = shop_id;
    record.seller_id = seller_id;
    record.shops_product_id = shops_product_id;
    record.product_id = product_id;
    record.title_native = TextRecord::new(native_title, native_language);

    let batch = Batch::try_from_iter(std::iter::once(record))
        .expect("shouldn't fail creating single-item batch");
    repository
        .transact_write_product_records_as_events(batch)
        .await
        .expect("shouldn't fail seeding product record");
}

#[localstack_test(services = [DynamoDB()])]
async fn should_persist_translated_title_events_when_domain_created_event_triggers_pipeline() {
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
        "Antiker Eichenstuhl",
        LanguageRecord::De,
    )
    .await;

    let get_product_service = GetProductServiceImpl::new(&repository);
    let domain_record =
        mk_domain_created_event_record(shop_id, seller_id, shops_product_id, product_id);

    let mut mock_service = MockTranslationService::new();
    mock_service
        .expect_translate()
        .once()
        .returning(|titles, _| {
            let count = titles.len();
            Box::pin(async move {
                vec![
                    Some(HashMap::from([
                        (Language::En, "Antique oak chair".to_string()),
                        (Language::Fr, "Chaise en chêne ancienne".to_string()),
                        (Language::Es, "Silla de roble antigua".to_string()),
                        (Language::It, "Sedia in rovere antico".to_string()),
                    ]));
                    count
                ]
            })
        });

    let event = mk_lambda_event(vec![mk_sqs_message(&domain_record)]);
    let result = handler(&mock_service, &get_product_service, &repository, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}

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

    let mut domain_messages = Vec::new();
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
            LanguageRecord::En,
        )
        .await;

        domain_messages.push(mk_sqs_message(&mk_domain_created_event_record(
            shop_id,
            seller_id,
            shops_product_id,
            product_id,
        )));
    }

    let mut mock_service = MockTranslationService::new();
    mock_service
        .expect_translate()
        .once()
        .returning(|titles, _| {
            let count = titles.len();
            Box::pin(async move {
                vec![
                    Some(HashMap::from([
                        (Language::De, "Antiker Stuhl".to_string()),
                        (Language::Fr, "Chaise ancienne".to_string()),
                        (Language::Es, "Silla antigua".to_string()),
                        (Language::It, "Sedia antica".to_string()),
                    ]));
                    count
                ]
            })
        });

    let event = mk_lambda_event(domain_messages);
    let result = handler(&mock_service, &get_product_service, &repository, event)
        .await
        .unwrap();

    assert!(
        result.batch_item_failures.is_empty(),
        "Expected no batch item failures but got: {:?}",
        result.batch_item_failures
    );
}
