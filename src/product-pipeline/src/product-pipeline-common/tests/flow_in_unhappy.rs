use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use product::dynamodb::product_event_record::ProductEventRecord;
use product_pipeline_common::flow_in::{FlowInResult, PipeFlowIn, PipeFlowInImpl};
use std::time::SystemTime;
use test_api::*;
use uuid::Uuid;

const SOURCE_QUEUE: Sqs = Sqs {
    name: "source-queue",
};

async fn prepare_messages(count: u16) {
    let sqs = get_sqs_client().await;
    for product_event_record in fake::vec![ProductEventRecord; count as usize] {
        sqs.send_message()
            .queue_url(SOURCE_QUEUE.queue_url())
            .message_body(mk_event_bridge_payload(&product_event_record))
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }
}

fn mk_event_bridge_payload(product_event_record: &ProductEventRecord) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(product_event_record).unwrap();
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

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_abort_when_queue_does_not_exist() {
    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, "non-existent-queue");

    let actual: FlowInResult = pipe_flow_in.flow_in(50, 100).await;

    assert!(actual.aborted);
    assert!(actual.data.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_handle_empty_queue_gracefully() {
    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());

    let actual: FlowInResult = pipe_flow_in.flow_in(50, 100).await;

    assert!(!actual.aborted);
    assert!(actual.data.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_handle_messages_with_invalid_json() {
    let sqs = get_sqs_client().await;

    // Send invalid JSON messages
    for _i in 0..5 {
        sqs.send_message()
            .queue_url(SOURCE_QUEUE.queue_url())
            .message_body("{invalid json}")
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }

    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let actual: FlowInResult = pipe_flow_in.flow_in(50, 100).await;

    assert!(!actual.aborted);
    // Invalid messages should be filtered out
    assert!(actual.data.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_handle_mixed_valid_and_invalid_messages() {
    prepare_messages(3).await;

    let sqs = get_sqs_client().await;

    // Add some invalid messages
    for _i in 0..2 {
        sqs.send_message()
            .queue_url(SOURCE_QUEUE.queue_url())
            .message_body("{invalid json}")
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }

    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let actual: FlowInResult = pipe_flow_in.flow_in(50, 100).await;

    assert!(!actual.aborted);
    // Only valid messages should be present
    assert_eq!(3, actual.data.len());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_respect_batch_limit() {
    prepare_messages(100).await;

    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());

    let actual: FlowInResult = pipe_flow_in.flow_in(25, 100).await;

    assert!(!actual.aborted);
    assert_eq!(25, actual.data.len());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_handle_very_small_visibility_timeout() {
    prepare_messages(5).await;

    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());

    // Use minimum visibility timeout
    let actual: FlowInResult = pipe_flow_in.flow_in(10, 1).await;

    assert!(!actual.aborted);
    assert_eq!(5, actual.data.len());
}
