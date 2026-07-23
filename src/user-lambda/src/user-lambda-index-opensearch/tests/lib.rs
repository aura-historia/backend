use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
    sqs::{SqsEvent, SqsMessage},
};
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use std::time::{Duration, SystemTime};
use test_api::*;
use user::{
    dynamodb::user_record::UserRecord,
    opensearch::{repository::UserOpenSearchRepositoryImpl, user_document::UserDocument},
};
use user_lambda_index_opensearch::handler;
use uuid::Uuid;

fn mk_sqs_message(record: &UserRecord) -> SqsMessage {
    let mut msg = SqsMessage::default();
    msg.message_id = Some(Faker.fake());
    msg.body = Some(mk_event_bridge_payload(record));
    msg
}

fn mk_event_bridge_payload(user_record: &UserRecord) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(user_record).unwrap();
    stream_record.size_bytes = 42;

    let mut event_record = EventRecord::default();
    event_record.aws_region = "eu-central-1".to_string();
    event_record.change = stream_record;
    event_record.event_id = Uuid::new_v4().to_string();
    event_record.event_name = "INSERT".to_string();

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "DynamoDBStreamRecord".to_string();
    event.source = "table".to_string();
    event.detail = event_record;
    serde_json::to_string(&event).unwrap()
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_index_user_document_when_not_exists() {
    let repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let user_record = Faker.fake::<UserRecord>();
    let mut lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    lambda_event.payload.records = vec![mk_sqs_message(&user_record)];

    let res = handler(&repository, lambda_event).await;

    assert!(res.is_ok());
    refresh_index("users").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let actual = read_by_id::<UserDocument>("users", user_record.user_id).await;
    assert_eq!(UserDocument::from(user_record), actual);
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_update_user_document_when_already_exists() {
    let repository = UserOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let user_record: UserRecord = Faker.fake();
    let mut lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    lambda_event.payload.records = vec![mk_sqs_message(&user_record)];
    let _ = handler(&repository, lambda_event).await;
    refresh_index("users").await;

    let mut updated_record = user_record.clone();
    updated_record.first_name = Some(Faker.fake());
    let mut update_event: LambdaEvent<SqsEvent> = LambdaEvent {
        payload: Default::default(),
        context: Context::default(),
    };
    update_event.payload.records = vec![mk_sqs_message(&updated_record)];

    let res = handler(&repository, update_event).await;

    assert!(res.is_ok());
    refresh_index("users").await;
    let actual = read_by_id::<UserDocument>("users", updated_record.user_id).await;
    assert_eq!(UserDocument::from(updated_record), actual);
}
