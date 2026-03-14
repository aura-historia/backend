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
use product::dynamodb::product_event_record::domain::{
    ProductDomainEventRecord, ProductDomainEventRecordSerdeField,
};
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

#[async_trait::async_trait]
impl PipeProcessor for FailingPipeProcessor {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
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

#[async_trait::async_trait]
impl PipeProcessor for AlwaysFailProcessor {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
        let failures = products.iter().map(|p| p.product_id).collect();
        ProcessResult {
            successes: Vec::new(),
            failures,
        }
    }
}

async fn prepare_messages(count: u16) -> Vec<ProductId> {
    let sqs = get_sqs_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let mut product_ids = vec![];
    for product_event_record in fake::vec![ProductEventRecord; count as usize] {
        product_ids.push(*product_event_record.product_id());
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
            ProductRecord::try_from(ProductDomainEventRecord::from(created_event)).unwrap();
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
    product_ids
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
    let product_ids = prepare_messages(10).await;
    let sqs = get_sqs_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);

    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = FailingPipeProcessor {
        fail_product_ids: product_ids.iter().take(3).copied().collect(),
    };
    let flow_out = PipeFlowOutImpl::new(&product_repository);
    let pipe: PipeImpl = PipeImpl::new(
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

    let actual_count = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|attr_value| {
            attr_value
                .get(ProductDomainEventRecordSerdeField::EventType.as_str())
                .cloned()
        })
        .count();
    assert_eq!(7, actual_count as u16);
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

    let actual_count = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|attr_value| {
            attr_value
                .get(ProductDomainEventRecordSerdeField::EventType.as_str())
                .cloned()
        })
        .count();
    assert_eq!(0, actual_count as u16);
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

    let actual_count = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|attr_value| {
            attr_value
                .get(ProductDomainEventRecordSerdeField::EventType.as_str())
                .cloned()
        })
        .count();
    assert_eq!(0, actual_count as u16);
}
