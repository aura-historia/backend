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

    let actual: FlowInResult<Dummy> = pipe_flow_in
        .flow_in(batch_in_count, visibility_timeout)
        .await;

    assert!(!actual.aborted);
    assert_eq!(batch_in_count.min(total_count) as usize, actual.data.len());
}
