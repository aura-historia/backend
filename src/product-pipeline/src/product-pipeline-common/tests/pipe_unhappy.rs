use common::product_id::ProductId;
use product_pipeline_common::flow_out::PipeFlowOutImpl;
use product_pipeline_common::pipe::Pipe;
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    pipe::PipeImpl,
    process::{PipeProcessor, ProcessResult},
    types::HasProductId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use test_api::*;

const SOURCE_QUEUE: Sqs = Sqs {
    name: "source-queue",
};
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

struct FailingPipeProcessor {
    fail_product_ids: HashSet<ProductId>,
}

impl PipeProcessor<Dummy, Dummy> for FailingPipeProcessor {
    fn process(&self, products: Vec<Dummy>) -> ProcessResult<Dummy> {
        let mut successes = Vec::new();
        let mut failures = HashSet::new();

        for product in products {
            if self.fail_product_ids.contains(&product.product_id) {
                failures.insert(product.product_id);
            } else {
                successes.push(product);
            }
        }

        ProcessResult {
            successes,
            failures,
        }
    }
}

struct AlwaysFailProcessor();

impl PipeProcessor<Dummy, Dummy> for AlwaysFailProcessor {
    fn process(&self, products: Vec<Dummy>) -> ProcessResult<Dummy> {
        let failures = products.iter().map(|p| p.product_id).collect();
        ProcessResult {
            successes: Vec::new(),
            failures,
        }
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
#[localstack_test(services = [SOURCE_QUEUE, TARGET_QUEUE])]
async fn should_handle_partial_processing_failures() {
    let products = fake::vec![Dummy; 10];
    let fail_ids: HashSet<ProductId> = products.iter().take(3).map(|p| p.product_id).collect();

    let sqs = get_sqs_client().await;
    for val in &products {
        sqs.send_message()
            .queue_url(SOURCE_QUEUE.queue_url())
            .message_body(serde_json::to_string(&val).unwrap())
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }

    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = FailingPipeProcessor {
        fail_product_ids: fail_ids.clone(),
    };
    let flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());
    let pipe: PipeImpl<'_, Dummy, Dummy, Dummy, Dummy> = PipeImpl::new(
        sqs,
        SOURCE_QUEUE.queue_url(),
        50,
        100,
        &flow_in,
        &processor,
        &flow_out,
    );

    pipe.pipe().await;

    // Check target queue has only successes
    let target_messages = sqs
        .receive_message()
        .queue_url(TARGET_QUEUE.queue_url())
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();

    // Should have 7 successful messages (10 - 3 failures)
    assert_eq!(7, target_messages.len());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE, TARGET_QUEUE])]
async fn should_handle_all_processing_failures() {
    prepare_messages(5).await;

    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = AlwaysFailProcessor();
    let flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());
    let pipe: PipeImpl<'_, Dummy, Dummy, Dummy, Dummy> = PipeImpl::new(
        sqs,
        SOURCE_QUEUE.queue_url(),
        50,
        100,
        &flow_in,
        &processor,
        &flow_out,
    );

    pipe.pipe().await;

    // Target queue should be empty (no successful processing)
    let target_messages = sqs
        .receive_message()
        .queue_url(TARGET_QUEUE.queue_url())
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();

    assert!(target_messages.is_empty());
}

#[rstest::rstest]
#[trace]
#[test_attr(apply(test))]
#[localstack_test(services = [SOURCE_QUEUE, TARGET_QUEUE])]
async fn should_handle_empty_queue_with_failing_processor() {
    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = AlwaysFailProcessor();
    let flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());
    let pipe: PipeImpl<'_, Dummy, Dummy, Dummy, Dummy> = PipeImpl::new(
        sqs,
        SOURCE_QUEUE.queue_url(),
        50,
        100,
        &flow_in,
        &processor,
        &flow_out,
    );

    pipe.pipe().await;

    // Both queues should be empty
    let source_messages = sqs
        .receive_message()
        .queue_url(SOURCE_QUEUE.queue_url())
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();

    let target_messages = sqs
        .receive_message()
        .queue_url(TARGET_QUEUE.queue_url())
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();

    assert!(source_messages.is_empty());
    assert!(target_messages.is_empty());
}
