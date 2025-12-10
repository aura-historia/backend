use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use common::{event::Event, event_id::EventId, product_id::ProductId};
use product::core::product_event::{ProductCreatedEventPayload, ProductEventPayload};
use product::dynamodb::{
    product_event_record::ProductEventRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use product_enrichment::pipeline::pipe::EnrichmentPipe;
use product_enrichment::{
    embed::EmbeddingDelegateImpl,
    pipeline::{
        embed::EmbeddingEnrichmentPipeImpl,
        faucet::EnrichmentPipeFaucetImpl,
        plumbing::{EnrichmentPlumbing, EnrichmentPlumbingImpl},
        sink::EnrichmentPipeSinkImpl,
    },
};
use std::{sync::Arc, time::SystemTime};
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const ENRICHMENT_QUEUE: Sqs = Sqs {
    name: "enrichment-queue",
};

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

#[trace]
#[rstest::rstest]
#[test_attr(apply(test))]
#[case(0, 0)]
#[case(1, 1)]
#[case(20, 15)]
#[localstack_test(services = [ENRICHMENT_QUEUE, DynamoDB(), OpenSearch()])]
async fn should_plumb_messages(#[case] queue_count: usize, #[case] plumbing_count: i32) {
    let enrichment_queue_url = ENRICHMENT_QUEUE.queue_url();
    let sqs_client = Arc::new(get_sqs_client().await.clone());
    let dynamodb_client = get_dynamodb_client().await;
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let event_records = fake::vec![ProductCreatedEventPayload; queue_count]
        .into_iter()
        .map(|payload| Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(payload),
        })
        .map(|event| ProductEventRecord::try_from(event).unwrap());
    for event_record in event_records.clone() {
        let payload = mk_event_bridge_payload(&event_record);
        let _ = sqs_client
            .send_message()
            .queue_url(&enrichment_queue_url)
            .message_body(payload)
            .delay_seconds(0)
            .send()
            .await
            .unwrap();

        // Simulate existing materialized views
        let ddb_put_res = product_dynamodb_repository
            .put_product_records([event_record.clone().try_into().unwrap()].into())
            .await
            .unwrap();
        assert!(ddb_put_res.unprocessed_items.unwrap_or_default().is_empty());
        let os_create_res = product_opensearch_repository
            .create_product_documents(vec![event_record.clone().try_into().unwrap()])
            .await
            .unwrap();
        assert!(!os_create_res.errors);
    }
    refresh_index("products").await;

    let faucet = EnrichmentPipeFaucetImpl::new(sqs_client.clone(), enrichment_queue_url.clone());
    let embedding_delegate = EmbeddingDelegateImpl::new().unwrap();
    let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(embedding_delegate));
    let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> = vec![Box::new(embedding_pipe)];
    let sink = EnrichmentPipeSinkImpl::new(
        Arc::new(product_dynamodb_repository),
        Arc::new(product_opensearch_repository),
    );

    let plumbing = EnrichmentPlumbingImpl::new(
        Arc::new(faucet),
        pipes,
        Arc::new(sink),
        sqs_client,
        enrichment_queue_url,
    );
    plumbing.plumb(plumbing_count).await;
}
