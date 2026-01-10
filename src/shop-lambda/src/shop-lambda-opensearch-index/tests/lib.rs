use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
    sqs::{SqsEvent, SqsMessage},
};
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use shop::{
    dynamodb::shop_record::ShopRecord,
    opensearch::{repository::ShopOpenSearchRepositoryImpl, shop_document::ShopDocument},
};
use shop_lambda_opensearch_index::handler;
use std::time::{Duration, SystemTime};
use test_api::*;
use uuid::Uuid;

fn mk_sqs_message(record: &ShopRecord) -> SqsMessage {
    let mut msg = SqsMessage::default();
    msg.message_id = Some(Faker.fake());
    msg.body = Some(mk_event_bridge_payload(record));
    msg
}

fn mk_event_bridge_payload(product_record: &ShopRecord) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(product_record).unwrap();
    stream_record.size_bytes = 42;

    let mut event_record = EventRecord::default();
    event_record.aws_region = "eu-central-1".to_string();
    event_record.change = stream_record;
    event_record.event_id = Uuid::new_v4().to_string();
    event_record.event_name = "INSERT".to_string();

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "foo".to_string();
    event.source = "bar".to_string();
    event.detail = event_record;

    serde_json::to_string(&event).unwrap()
}

#[localstack_test(services = [OpenSearch()])]
async fn should_index_shop_document_when_not_exists() {
    let repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let shop_record = Faker.fake::<ShopRecord>();
    let mut lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    lambda_event.payload.records = vec![mk_sqs_message(&shop_record)];

    let res = handler(&repository, lambda_event).await;
    assert!(res.is_ok());
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let actual = read_by_id::<ShopDocument>("shops", shop_record.shop_id).await;
    assert_eq!(ShopDocument::from(shop_record), actual);
}

#[localstack_test(services = [OpenSearch()])]
async fn should_index_shop_document_when_exists() {
    let repository = ShopOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    // First invocation
    let shop_record = Faker.fake::<ShopRecord>();
    let mut lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    lambda_event.payload.records = vec![mk_sqs_message(&shop_record)];

    let res = handler(&repository, lambda_event).await;
    assert!(res.is_ok());
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let actual = read_by_id::<ShopDocument>("shops", shop_record.shop_id).await;
    assert_eq!(ShopDocument::from(shop_record.clone()), actual);

    // Second invocation, different name
    let mut shop_record = shop_record;
    shop_record.name = "Dingel dings bums".into();
    let mut lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    lambda_event.payload.records = vec![mk_sqs_message(&shop_record)];

    let res = handler(&repository, lambda_event).await;
    assert!(res.is_ok());
    refresh_index("shops").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let actual = read_by_id::<ShopDocument>("shops", shop_record.shop_id).await;
    assert_eq!(ShopDocument::from(shop_record.clone()), actual);
}
