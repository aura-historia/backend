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

struct Const42PipeProcessor();
impl PipeProcessor<Dummy, Dummy> for Const42PipeProcessor {
    fn process(&self, products: Vec<Dummy>) -> ProcessResult<Dummy> {
        ProcessResult {
            successes: products
                .into_iter()
                .map(|mut product| {
                    product.moo = 42;
                    product.bar = "foo".into();
                    product
                })
                .collect(),
            failures: HashSet::new(),
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
#[case(0, 50, 100)]
#[case(1, 50, 100)]
#[case(49, 50, 100)]
#[case(50, 50, 100)]
#[case(69, 50, 100)]
#[case(99, 50, 100)]
#[case(100, 50, 100)]
#[case(0, 27, 100)]
#[case(1, 27, 100)]
#[case(49, 27, 100)]
#[case(50, 27, 100)]
#[case(69, 27, 100)]
#[case(99, 27, 100)]
#[case(100, 500, 300)]
#[localstack_test(services = [SOURCE_QUEUE, TARGET_QUEUE])]
async fn should_pipe_messages(
    #[case] total_count: u16,
    #[case] batch_in_count: u16,
    #[case] visibility_timeout: u16,
) {
    prepare_messages(total_count).await;
    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = Const42PipeProcessor();
    let flow_out = PipeFlowOutImpl::new(sqs, TARGET_QUEUE.queue_url());
    let pipe: PipeImpl<'_, Dummy, Dummy, Dummy, Dummy> = PipeImpl::new(
        sqs,
        SOURCE_QUEUE.queue_url(),
        batch_in_count,
        visibility_timeout,
        &flow_in,
        &processor,
        &flow_out,
    );

    pipe.pipe().await;

    let source_queue_empty = sqs
        .receive_message()
        .queue_url(SOURCE_QUEUE.queue_url())
        .max_number_of_messages(10)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default()
        .is_empty();
    assert!(source_queue_empty);

    let mut target_queue_messages = Vec::with_capacity(total_count as usize);
    loop {
        let messages = sqs
            .receive_message()
            .queue_url(TARGET_QUEUE.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap()
            .messages
            .unwrap_or_default();
        if messages.is_empty() {
            break;
        } else {
            target_queue_messages.extend(messages);
        }
    }
    assert_eq!(total_count as usize, target_queue_messages.len());
    let target_queue_messages_all_processed = target_queue_messages
        .into_iter()
        .map(|msg| serde_json::from_str::<Dummy>(&msg.body.unwrap()).unwrap())
        .all(|val| &val.bar == "foo" && val.moo == 42);
    assert!(target_queue_messages_all_processed);
}
