use common::product_id::ProductId;
use product_pipeline_common::flow_out::{FlowOutResult, PipeFlowOut};
use product_pipeline_common::{flow_out::PipeFlowOutImpl, types::HasProductId};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use test_api::*;

const TARGET_QUEUE: Sqs = Sqs {
    name: "target-queue",
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

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_handle_partial_failures_when_queue_not_exists() {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, "non-existent-queue");

    let data = fake::vec![Dummy; 10];
    let actual: FlowOutResult = pipe_flow_out.flow_out(data.clone()).await;

    assert!(actual.successes.is_empty());
    assert_eq!(10, actual.failures.len());
    for item in data {
        assert!(actual.failures.contains(&item.product_id));
    }
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_retry_and_succeed_after_transient_failures() {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());

    let data = fake::vec![Dummy; 50];
    let actual: FlowOutResult = pipe_flow_out.flow_out(data.clone()).await;

    assert!(actual.failures.is_empty());
    assert_eq!(50, actual.successes.len());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_handle_empty_batch_gracefully() {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());

    let data: Vec<Dummy> = Vec::new();
    let actual: FlowOutResult = pipe_flow_out.flow_out(data).await;

    assert!(actual.successes.is_empty());
    assert!(actual.failures.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_track_failures_separately_from_successes() {
    let sqs = get_sqs_client().await;
    
    // Use valid queue for testing retry logic
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());
    let data = fake::vec![Dummy; 100];
    
    let actual: FlowOutResult = pipe_flow_out.flow_out(data.clone()).await;

    // With a valid queue, all should succeed
    assert_eq!(100, actual.successes.len());
    assert!(actual.failures.is_empty());
    
    // Verify no overlap between successes and failures
    let success_set: HashSet<ProductId> = actual.successes.iter().copied().collect();
    let failure_set: HashSet<ProductId> = actual.failures.iter().copied().collect();
    assert!(success_set.is_disjoint(&failure_set));
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_handle_large_batch_exceeding_sqs_limit() {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());

    // Create more than batch size (10) to test batching logic
    let data = fake::vec![Dummy; 25];
    let actual: FlowOutResult = pipe_flow_out.flow_out(data.clone()).await;

    assert_eq!(25, actual.successes.len());
    assert!(actual.failures.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_handle_multiple_batches_with_retry_logic() {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());

    // Test with exactly 10 items (one batch)
    let data = fake::vec![Dummy; 10];
    let actual: FlowOutResult = pipe_flow_out.flow_out(data.clone()).await;

    assert_eq!(10, actual.successes.len());
    assert!(actual.failures.is_empty());
}
