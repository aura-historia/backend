use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::has_key::HasKey;
use common::product_id::ProductKey;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::dynamodb::repository::ProductDynamoDbRepository;
use product_lambda_common::extract_product_event_record;
use tracing::{error, info};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut updates = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ProductKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(update) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            updates.push(update);
        }
    }

    for (key, update) in updates {
        let update_res = repository
            .update_product_record(&key.shop_id, &key.shops_product_id, update)
            .await;
        if let Err(err) = update_res {
            error!(error = ?err, productKey = %key, "Failed update.");
            match message_ids.remove(&key) {
                Some(message_id) => failed_message_ids.push(message_id),
                None => {
                    error!(
                        productKey = %key,
                        "There exists no message_id for failed ProductRecord."
                    );
                }
            }
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count - skipped_count,
        failures = failure_count,
        skipped = skipped_count,
        "Handler finished.",
    );
    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(sqs_batch_response)
}

fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
    message_ids: &mut HashMap<ProductKey, String>,
) -> Option<(ProductKey, ProductRecordUpdate)> {
    let message_id = message
        .message_id
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let product_event_record =
        extract_product_event_record(message, failed_message_ids, skipped_count)?;
    let key = product_event_record.key();
    let update_record = ProductRecordUpdate::from(product_event_record);
    message_ids.insert(key.clone(), message_id);
    Some((key, update_record))
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::update_item::UpdateItemOutput;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::{ProductCommonEventPayload, ProductEvent};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use std::time::SystemTime;
    use uuid::Uuid;

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

    fn mk_sqs_message(product_event_record: &ProductEventRecord) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(product_event_record));
        msg
    }

    fn mk_sqs_message_with_id(product_event_record: &ProductEventRecord, message_id: String) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(product_event_record));
        msg
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    async fn should_handle_sqs_message(#[case] record_count: usize) {
        let records = fake::vec![ProductEvent; record_count]
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|product_event_record| mk_sqs_message(&product_event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_update_product_record()
            .returning(move |_, _, _| {
                Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
            });

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(7, 10)]
    #[case(24, 25)]
    #[case(0, 47)]
    #[case(98, 100)]
    #[case(1, 150)]
    #[case(0, 453)]
    #[case(0, 900)]
    #[case(2874, 2874)]
    #[case(874, 10874)]
    async fn should_respond_with_partial_failures(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let events = fake::vec![ProductEvent; record_count];
        let expected_failed_events = events
            .clone()
            .into_iter()
            .take(failure_count)
            .collect::<Vec<_>>();
        let mut expected_failed_message_ids = Vec::with_capacity(failure_count);
        let records = events
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let message_id = Uuid::new_v4().to_string();
                if expected_failed_events.iter().any(|event| {
                    event.payload.shop_id() == &event_record.shop_id
                        && event.payload.shops_product_id() == &event_record.shops_product_id
                }) {
                    expected_failed_message_ids.push(message_id.clone());
                }
                mk_sqs_message_with_id(&event_record, message_id)
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_update_product_record()
            .returning(move |shop_id, shops_product_id, _| {
                if expected_failed_events.iter().any(|event| {
                    event.payload.shop_id() == shop_id
                        && event.payload.shops_product_id() == shops_product_id
                }) {
                    Box::pin(
                        async move { Err(SdkError::construction_failure("Something went wrong.")) },
                    )
                } else {
                    Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
                }
            });

        expected_failed_message_ids.sort();
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}
