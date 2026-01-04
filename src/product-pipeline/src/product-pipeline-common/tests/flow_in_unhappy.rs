use common::product_id::ProductId;
use product_pipeline_common::{
    flow_in::{FlowInResult, PipeFlowIn, PipeFlowInImpl},
    types::HasProductId,
};
use serde::{Deserialize, Serialize};
use test_api::*;

const SOURCE_QUEUE: Sqs = Sqs {
    name: "source-queue",
};

#[derive(Debug, Clone, Serialize, Deserialize, fake::Dummy)]
struct Dummy {
    moo: u64,
    bar: String,
    product_id: ProductId,
}

impl HasProductId for Dummy {
    fn product_id(&self) -> ProductId {
        self.product_id
    }
}

async fn prepare_messages(count: u16) {
    let sqs = get_sqs_client().await;
    for val in fake::vec![Dummy; count as usize] {
        sqs.send_message()
            .queue_url(SOURCE_QUEUE.queue_url())
            .message_body(serde_json::to_string(&val).unwrap())
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE])]
async fn should_abort_when_queue_does_not_exist() {
    let sqs = get_sqs_client().await;
    let pipe_flow_in = PipeFlowInImpl::new(sqs, "non-existent-queue");

    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(50, 100).await;

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

    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(50, 100).await;

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
    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(50, 100).await;

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
            .message_body("{not valid json at all}")
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }

    let pipe_flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(50, 100).await;

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

    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(25, 100).await;

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
    let actual: FlowInResult<Dummy> = pipe_flow_in.flow_in(10, 1).await;

    assert!(!actual.aborted);
    assert_eq!(5, actual.data.len());
}
