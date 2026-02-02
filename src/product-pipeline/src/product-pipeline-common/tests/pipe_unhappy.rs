use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::product_id::ProductId;
use fake::{Fake, Faker};
use product::core::product::Product;
use product::core::product_event::domain::{
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::get_service::GetProductServiceImpl;
use product_pipeline_common::flow_out::PipeFlowOutImpl;
use product_pipeline_common::pipe::Pipe;
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    pipe::PipeImpl,
    process::{PipeProcessor, ProcessResult},
};
use std::collections::HashSet;
use std::time::SystemTime;
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const SOURCE_QUEUE: Sqs = Sqs {
    name: "source-queue",
};

struct FailingPipeProcessor {
    fail_product_ids: HashSet<ProductId>,
}

impl PipeProcessor for FailingPipeProcessor {
    fn process(&self, products: Vec<Product>) -> ProcessResult {
        let mut successes = Vec::new();
        let mut failures = HashSet::new();

        for product in products {
            if self.fail_product_ids.contains(&product.product_id) {
                failures.insert(product.product_id);
            } else {
                successes.push(Faker.fake());
            }
        }

        ProcessResult {
            successes,
            failures,
        }
    }
}

struct AlwaysFailProcessor();

impl PipeProcessor for AlwaysFailProcessor {
    fn process(&self, products: Vec<Product>) -> ProcessResult {
        let failures = products.iter().map(|p| p.product_id).collect();
        ProcessResult {
            successes: Vec::new(),
            failures,
        }
    }
}

async fn prepare_messages(count: u16) {
    let sqs = get_sqs_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    for product_event_record in fake::vec![ProductEventRecord; count as usize] {
        let mut product_created_event_payload: ProductCreatedDomainEventPayload = Faker.fake();
        product_created_event_payload.shop_id = product_event_record.key().shop_id;
        product_created_event_payload.shops_product_id =
            product_event_record.key().shops_product_id;
        let created_event = Event {
            aggregate_id: *product_event_record.product_id(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(product_created_event_payload),
        };
        let product_record =
            ProductRecord::try_from(ProductDomainEventRecord::try_from(created_event).unwrap())
                .unwrap();
        product_repository
            .put_product_records([product_record].into())
            .await
            .unwrap();

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
#[localstack_test(services = [DynamoDB(), SOURCE_QUEUE])]
async fn should_handle_partial_processing_failures() {
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);

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
    let flow_out = PipeFlowOutImpl::new(&product_repository);
    let pipe = PipeImpl::new(
        &get_product_service,
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
#[localstack_test(services = [DynamoDB(), SOURCE_QUEUE])]
async fn should_handle_all_processing_failures() {
    prepare_messages(5).await;

    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = AlwaysFailProcessor();
    let flow_out = PipeFlowOutImpl::new(&product_repository);
    let pipe = PipeImpl::new(
        &get_product_service,
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
#[localstack_test(services = [DynamoDB(), SOURCE_QUEUE])]
async fn should_handle_empty_queue_with_failing_processor() {
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = AlwaysFailProcessor();
    let flow_out = PipeFlowOutImpl::new(&product_repository);
    let pipe = PipeImpl::new(
        &get_product_service,
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
