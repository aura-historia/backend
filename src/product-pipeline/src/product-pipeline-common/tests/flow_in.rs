use std::time::SystemTime;

use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use product::dynamodb::product_event_record::ProductEventRecord;
use product_pipeline_common::flow_in::{FlowInResult, PipeFlowIn, PipeFlowInImpl};
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
#[case(0, 50, 100)]
#[case(1, 50, 100)]
#[case(49, 50, 100)]
#[case(50, 50, 100)]
#[case(69, 50, 100)]
#[case(99, 50, 100)]
#[case(100, 50, 100)]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_flow_in_messages(
    #[case] total_count: u16,
    #[case] batch_in_count: u16,
    #[case] visibility_timeout: u16,
) {
    prepare_messages(total_count).await;
    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());

    let actual: FlowInResult = pipe_flow_in
        .flow_in(batch_in_count, visibility_timeout)
        .await;

    assert!(!actual.aborted);
    assert_eq!(batch_in_count.min(total_count) as usize, actual.data.len());
}
