use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::batch::dynamodb::handle_dynamodb_batch_write_put_item_output;
use common::product_id::ProductKey;
use common::{batch::Batch, has_key::HasKey};
use lambda_runtime::LambdaEvent;
use product::dynamodb::{
    item_event_record::ProductEventRecord, repository::ProductDynamoDbRepository,
};
use std::collections::HashMap;
use tracing::{error, info};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut message_ids: HashMap<ProductKey, String> = HashMap::with_capacity(records_count);

    let event_records = event
        .payload
        .records
        .into_iter()
        .filter_map(|msg| extract_message_data(msg, &mut failed_message_ids, &mut message_ids));
    let batches = Batch::<_, 25>::chunked_from(event_records);
    let mut failed_keys = Vec::new();
    for batch in batches {
        let item_keys = batch
            .iter()
            .map(ProductEventRecord::key)
            .collect::<Vec<_>>();
        let put_batch_res = repository.put_item_event_records(batch).await;
        match put_batch_res {
            Ok(output) => handle_dynamodb_batch_write_put_item_output::<ProductEventRecord>(
                output,
                &mut failed_keys,
            ),
            Err(err) => {
                error!(error = ?err, "Failed writing entire ProductEventRecord-Batch due to SdkError.");
                failed_keys.extend(item_keys);
            }
        }
    }

    for failed_command_key in failed_keys {
        let message_id = message_ids.remove(&failed_command_key);
        match message_id {
            Some(message_id) => failed_message_ids.push(message_id),
            None => {
                error!(
                    itemKey = failed_command_key.to_string(),
                    "There exists no message_id for a failed command."
                );
            }
        }
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count,
        failures = failure_count,
        "Handler finished.",
    );
    let sqs_batch_response = SqsBatchResponse {
        batch_item_failures: failed_message_ids
            .into_iter()
            .map(|item_identifier| BatchItemFailure { item_identifier })
            .collect(),
    };

    Ok(sqs_batch_response)
}

fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    message_ids: &mut HashMap<ProductKey, String>,
) -> Option<ProductEventRecord> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            failed_message_ids.push(message_id);
            None
        }
        Some(json) => match serde_json::from_str::<ProductEventRecord>(&json) {
            Ok(event_record) => {
                message_ids.insert(event_record.key(), message_id);
                Some(event_record)
            }
            Err(e) => {
                error!(
                    error = %e,
                    messageId = message_id,
                    payload = %json,
                    type = %std::any::type_name::<ProductEventRecord>(),
                    "Failed deserializing."
                );
                failed_message_ids.push(message_id);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::{
        operation::batch_write_item::BatchWriteItemOutput,
        types::{PutRequest, WriteRequest},
    };
    use common::has_key::HasKey;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::dynamodb::{
        item_event_record::ProductEventRecord, repository::MockItemDynamoDbRepository,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

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
    #[tokio::test]
    async fn should_handle_message(#[case] record_count: usize) {
        let records = fake::vec![ProductEventRecord; record_count]
            .into_iter()
            .map(|record| serde_json::to_string(&record))
            .map(Result::unwrap)
            .map(|json_payload| SqsMessage {
                message_id: Some(Faker.fake()),
                receipt_handle: None,
                body: Some(json_payload),
                md5_of_body: None,
                md5_of_message_attributes: None,
                attributes: Default::default(),
                message_attributes: Default::default(),
                event_source_arn: None,
                event_source: None,
                aws_region: None,
            })
            .collect();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };

        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_put_item_event_records()
            .returning(|_| Box::pin(async { Ok(BatchWriteItemOutput::builder().build()) }));
        let response = handler(&repository, lambda_event).await.unwrap();

        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(9, 10)]
    #[case(0, 25)]
    async fn should_respond_with_partial_failures(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let event_records = fake::vec![ProductEventRecord; record_count];
        let expected_failed_keys = event_records
            .iter()
            .take(failure_count)
            .map(ProductEventRecord::key)
            .collect::<Vec<_>>();
        let mut messages_ids = HashMap::with_capacity(record_count);
        let records = event_records
            .into_iter()
            .map(|cmd_data| {
                let message_id = Uuid::new_v4().to_string();
                messages_ids.insert(cmd_data.key(), message_id.clone());
                SqsMessage {
                    message_id: Some(message_id),
                    receipt_handle: None,
                    body: Some(serde_json::to_string(&cmd_data).unwrap()),
                    md5_of_body: None,
                    md5_of_message_attributes: None,
                    attributes: Default::default(),
                    message_attributes: Default::default(),
                    event_source_arn: None,
                    event_source: None,
                    aws_region: None,
                }
            })
            .collect();
        let mut expected_failed_message_ids = expected_failed_keys
            .iter()
            .map(|key| messages_ids.remove(key).unwrap())
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };

        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_put_item_event_records()
            .return_once(move |_| {
                let write_requests = expected_failed_keys
                    .into_iter()
                    .map(|key| {
                        let mut fake = Faker.fake::<ProductEventRecord>();
                        fake.shop_id = key.shop_id;
                        fake.shops_product_id = key.shops_product_id;
                        WriteRequest::builder()
                            .put_request(
                                PutRequest::builder()
                                    .set_item(Some(serde_dynamo::to_item(fake).unwrap()))
                                    .build()
                                    .unwrap(),
                            )
                            .build()
                    })
                    .collect();
                Box::pin(async {
                    Ok(BatchWriteItemOutput::builder()
                        .unprocessed_items("table_1", write_requests)
                        .build())
                })
            });
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(failure_count, actual_failed_message_ids.len());
        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}
