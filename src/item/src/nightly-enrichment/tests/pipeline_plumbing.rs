use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use common::{event::Event, event_id::EventId, item_id::ItemId};
use item_core::item_event::{ItemCreatedEventPayload, ItemEventPayload};
use item_dynamodb::{item_event_record::ItemEventRecord, repository::ItemDynamoDbRepositoryImpl};
use item_opensearch::repository::ItemOpenSearchRepositoryImpl;
use nightly_enrichment::{
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

fn mk_event_bridge_payload(item_event_record: &ItemEventRecord) -> String {
    let event = EventBridgeEvent {
        version: None,
        id: None,
        detail_type: "foo".to_string(),
        source: "bar".to_string(),
        account: None,
        time: None,
        region: None,
        resources: None,
        detail: EventRecord {
            aws_region: "eu-central-1".to_string(),
            change: StreamRecord {
                approximate_creation_date_time: SystemTime::now().into(),
                keys: Default::default(),
                new_image: serde_dynamo::to_item(item_event_record).unwrap(),
                old_image: Default::default(),
                sequence_number: None,
                size_bytes: 42,
                stream_view_type: None,
            },
            event_id: Uuid::new_v4().to_string(),
            event_name: "INSERT".to_string(),
            event_source: None,
            event_version: None,
            event_source_arn: None,
            user_identity: None,
            record_format: None,
            table_name: None,
        },
    };
    serde_json::to_string(&event).unwrap()
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(0, 0)]
#[case(1, 1)]
#[case(20, 15)]
#[case(1000, 500)]
#[case(2000, 855)]
#[localstack_test(services = [ENRICHMENT_QUEUE])]
async fn should_pour_messages(#[case] queue_count: usize, #[case] plumbing_count: i32) {
    use nightly_enrichment::pipeline::pipe::EnrichmentPipe;

    let enrichment_queue_url = ENRICHMENT_QUEUE.queue_url();
    let sqs_client = Arc::new(get_sqs_client().await.clone());

    let event_records = fake::vec![ItemCreatedEventPayload; queue_count]
        .into_iter()
        .map(|payload| Event {
            aggregate_id: ItemId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(payload),
        })
        .map(|event| ItemEventRecord::try_from(event).unwrap());
    for event_record in event_records {
        let payload = mk_event_bridge_payload(&event_record);
        let _ = sqs_client
            .send_message()
            .queue_url(&enrichment_queue_url)
            .message_body(payload)
            .delay_seconds(0)
            .send()
            .await
            .unwrap();
    }

    let faucet = EnrichmentPipeFaucetImpl::new(sqs_client.clone(), enrichment_queue_url.clone());
    let embedding_delegate = EmbeddingDelegateImpl::new().unwrap();
    let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(embedding_delegate));
    let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> = vec![Box::new(embedding_pipe)];
    let dynamodb_client = get_dynamodb_client().await;
    let item_dynamodb_repository = ItemDynamoDbRepositoryImpl::new(dynamodb_client, "table_1");
    let item_opensearch_repository =
        ItemOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let sink = EnrichmentPipeSinkImpl::new(
        Arc::new(item_dynamodb_repository),
        Arc::new(item_opensearch_repository),
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
