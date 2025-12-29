use common::product_id::ProductId;
use product_pipeline_common::flow_out::{FlowOutResult, PipeFlowOut};
use product_pipeline_common::{flow_out::PipeFlowOutImpl, types::HasProductId};
use serde::{Deserialize, Serialize};
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
#[case(0)]
#[case(1)]
#[case(42)]
#[case(49)]
#[case(50)]
#[case(69)]
#[case(99)]
#[case(100)]
#[localstack_test(services = [TARGET_QUEUE])]
async fn should_flow_out_messages(#[case] total_count: u16) {
    let sqs = get_sqs_client().await;
    let pipe_flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());

    let actual: FlowOutResult = pipe_flow_out
        .flow_out(fake::vec![Dummy; total_count as usize])
        .await;

    assert!(actual.failures.is_empty());
    assert_eq!(total_count as usize, actual.successes.len());
}
