use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use common::{event::Event, event_id::EventId, product_id::ProductId};
use product::core::product_event::{ProductCreatedEventPayload, ProductEventPayload};
use product::dynamodb::product_event_record::ProductEventRecord;
use product_enrichment::pipeline::faucet::{EnrichmentPipeFaucet, EnrichmentPipeFaucetImpl};
use std::{sync::Arc, time::SystemTime};
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const ENRICHMENT_QUEUE: Sqs = Sqs {
    name: "enrichment-queue",
};

fn mk_event_bridge_payload(product_event_record: &ProductEventRecord) -> String {
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
                new_image: serde_dynamo::to_item(product_event_record).unwrap(),
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
async fn should_pour_messages(#[case] queue_count: usize, #[case] pour_count: i32) {
    let enrichment_queue_url = ENRICHMENT_QUEUE.queue_url();
    let sqs_client = Arc::new(get_sqs_client().await.clone());

    let event_records = fake::vec![ItemCreatedEventPayload; queue_count]
        .into_iter()
        .map(|payload| Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(payload),
        })
        .map(|event| ProductEventRecord::try_from(event).unwrap());
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

    let faucet = EnrichmentPipeFaucetImpl::new(sqs_client, enrichment_queue_url);
    let actual = faucet.pour(pour_count).await;

    assert_eq!(pour_count as usize, actual.len());
    assert!(
        actual
            .iter()
            .all(|(pipe_item, msg_ref)| pipe_item.source.product_id == msg_ref.product_id)
    );
    assert!(
        actual
            .iter()
            .all(|(pipe_item, _)| pipe_item.update.document.is_none())
    );
    assert!(
        actual
            .iter()
            .all(|(pipe_item, _)| pipe_item.update.record.is_none())
    );
}
