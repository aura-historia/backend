use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::dynamodb_stream::extract_sqs_event_bridge_dynamodb_record;
use common::opensearch::bulk_response::{BulkItemResult, BulkResponse};
use common::product_id::ProductId;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::opensearch::product_update_document::ProductUpdateDocument;
use product::opensearch::repository::ProductOpenSearchRepository;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ProductOpenSearchRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut update_documents = HashMap::with_capacity(records_count);
    let mut message_ids: HashMap<ProductId, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some((product_id, product_document)) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            update_documents.insert(product_id, product_document);
        }
    }

    let result = repository.update_product_documents(update_documents).await;
    match result {
        Ok(response) => handle_bulk_response(response, &mut failed_message_ids, &mut message_ids),
        Err(err) => {
            error!(error = ?err, "Failed entire batch.");
            failed_message_ids.extend(message_ids.into_values());
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
    message_ids: &mut HashMap<ProductId, String>,
) -> Option<(ProductId, ProductUpdateDocument)> {
    let message_id = message
        .message_id
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let product_event_record: ProductDomainEventRecord =
        extract_sqs_event_bridge_dynamodb_record(message, failed_message_ids, skipped_count)?;
    let product_id = product_event_record.product_id;
    let update_document = ProductUpdateDocument::from(product_event_record);
    message_ids.insert(product_id, message_id);
    Some((product_id, update_document))
}

fn handle_bulk_response(
    response: BulkResponse,
    failed_message_ids: &mut Vec<String>,
    message_ids: &mut HashMap<ProductId, String>,
) {
    if response.errors {
        let failures = response
            .items
            .into_iter()
            .filter_map(|bulk_product_result| match bulk_product_result {
                BulkItemResult::Update { update } => Some(update),
                other => {
                    error!(actual = ?other, "Expected BulkItemResult::Update.");
                    None
                }
            })
            .filter(|bulk_op_result| bulk_op_result.is_err());

        for failure in failures {
            warn!(
                index = failure.index,
                productId = failure.id,
                status = failure.status,
                error = ?failure.error,
                "Failed updating product in OpenSearch."
            );
            match ProductId::try_from(failure.id.as_str()) {
                Ok(product_id) => match message_ids.remove(&product_id) {
                    Some(message_id) => {
                        failed_message_ids.push(message_id);
                    }
                    None => {
                        error!(
                            index = failure.index,
                            productId = failure.id,
                            "Failed re-mapping product-id to message-id. Cannot retry."
                        );
                    }
                },
                Err(err) => {
                    error!(
                        index = failure.index,
                        productId = failure.id,
                        error = %err,
                        payload = ?failure,
                        "Failed parsing '_id' from OpenSearch-Response as 'ProductId'. Cannot retry."
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use common::opensearch::bulk_response::BulkItemResult;
    use common::opensearch::bulk_response::BulkOpResult;
    use common::opensearch::bulk_response::{BulkError, BulkResponse};
    use fake::Fake;
    use fake::Faker;
    use lambda_runtime::LambdaEvent;
    use product::core::product_event::ProductDomainEvent;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::opensearch::repository::MockProductOpenSearchRepository;
    use std::collections::HashMap;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn mk_event_bridge_payload(product_event_record: &ProductDomainEventRecord) -> String {
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

    fn mk_sqs_message(event_record: &ProductDomainEventRecord) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(event_record));
        msg
    }

    fn mk_sqs_message_with_id(
        event_record: &ProductDomainEventRecord,
        message_id: String,
    ) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(event_record));
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
    #[trace]
    async fn should_handle_message(#[case] record_count: usize) {
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_update_product_documents()
            .return_once(|_| {
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: false,
                        items: vec![],
                    })
                })
            });

        let records = fake::vec![ProductCreatedDomainEventPayload; record_count]
            .into_iter()
            .map(ProductDomainEventPayload::Created)
            .map(|event_payload| Event {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            })
            .map(ProductDomainEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| mk_sqs_message(&event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
            payload: sqs_event,
            context: Default::default(),
        };

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty())
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(9, 10)]
    #[case(0, 25)]
    #[case(47, 47)]
    #[case(100, 100)]
    #[case(0, 150)]
    #[case(234, 453)]
    #[case(773, 900)]
    #[case(299, 2874)]
    #[case(77, 10874)]
    #[trace]
    async fn should_respond_with_partial_failures_when_opensearch_partial_bulk_failure(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
        let expected_failures = message_ids
            .keys()
            .take(failure_count)
            .cloned()
            .collect::<Vec<_>>();
        let expected_failures_clone = expected_failures.clone();
        let records = fake::vec![ProductDomainEvent; record_count]
            .into_iter()
            .map(ProductDomainEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(event_record.product_id, uuid.clone());
                mk_sqs_message_with_id(&event_record, uuid)
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Default::default(),
        };
        let mut repository = MockProductOpenSearchRepository::default();
        repository
            .expect_update_product_documents()
            .return_once(move |batch| {
                let failures: Vec<_> = batch
                    .iter()
                    .filter(|(product_id, _)| expected_failures_clone.contains(product_id))
                    .map(|(product_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: product_id.to_string(),
                            version: Some(2),
                            status: 409,
                            error: Some(BulkError {
                                error_type: "boop".to_string(),
                                reason: "[items][3]: version conflict, document doesn't exist"
                                    .to_string(),
                                index_uuid: Some(Uuid::new_v4().to_string()),
                                shard: Some("shard-1".to_string()),
                                index: Some(index),
                                extra: Default::default(),
                            }),
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                let successes: Vec<_> = batch
                    .iter()
                    .filter(|(product_id, _)| !expected_failures_clone.contains(product_id))
                    .map(|(product_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: product_id.to_string(),
                            version: Some(2),
                            status: 200,
                            error: None,
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: true,
                        items: [successes, failures].concat(),
                    })
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
        let mut expected_failed_message_ids = expected_failures
            .into_iter()
            .map(|product_id| message_ids.remove(&product_id))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}
