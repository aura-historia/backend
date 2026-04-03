use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use fake::{Fake, Faker};
use product::core::product::Product;
use product::core::product_event::ProductEvent;
use product::core::product_event::domain::{
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
};
use product::core::product_event::enrichment::{
    ProductEnrichmentEventPayload, TranslationProductEnrichmentEventPayload,
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

struct Const42PipeProcessor();
#[async_trait::async_trait]
impl PipeProcessor for Const42PipeProcessor {
    async fn process(&self, products: Vec<Product>) -> ProcessResult {
        ProcessResult {
            successes: products
                .into_iter()
                .map(|product| ProductEvent {
                    aggregate_id: product.product_id,
                    event_id: product.event_id,
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ProductEnrichmentEventPayload::TranslatedTitle(
                        TranslationProductEnrichmentEventPayload {
                            shop_id: product.shop_id,
                            seller_id: product.seller_id,
                            shops_product_id: product.shops_product_id,
                            source_language: common::language::domain::Language::De,
                            target_language: common::language::domain::Language::En,
                            target: "42".into(),
                        },
                    )
                    .into(),
                })
                .collect(),
            failures: HashSet::new(),
        }
    }
}

async fn prepare_messages(count: u16) {
    let sqs = get_sqs_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    for product_event_record in fake::vec![ProductEventRecord; count as usize] {
        let mut product_created_event_payload = Faker.fake::<ProductCreatedDomainEventPayload>();
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
#[case(0, 27, 100)]
#[case(1, 27, 100)]
#[case(49, 27, 100)]
#[case(50, 27, 100)]
#[case(69, 27, 100)]
#[case(99, 27, 100)]
#[case(100, 500, 300)]
#[localstack_test(services = [DynamoDB(), SOURCE_QUEUE])]
async fn should_pipe_messages(
    #[case] total_count: u16,
    #[case] batch_in_count: u16,
    #[case] visibility_timeout: u16,
) {
    prepare_messages(total_count).await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let sqs = get_sqs_client().await;
    let flow_in = PipeFlowInImpl::new(sqs, SOURCE_QUEUE.queue_url());
    let processor = Const42PipeProcessor();
    let flow_out = PipeFlowOutImpl::new(&product_repository);
    let pipe: PipeImpl = PipeImpl::new(
        &get_product_service,
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
    assert_eq!(total_count, actual_count as u16);
}
