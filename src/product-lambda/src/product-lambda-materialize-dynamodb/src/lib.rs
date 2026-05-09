use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::batch::Batch;
use common::batch::dynamodb::handle_dynamodb_batch_write_put_product_output;
use common::dynamodb_stream::extract_from_dynamodb_stream;
use common::has_key::HasKey;
use common::product_id::ProductKey;
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::dynamodb::repository::ProductDynamoDbRepository;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(
        product_repository,
        event
    ),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    product_repository: &impl ProductDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");
    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut creates: HashMap<String, ProductRecord> = HashMap::new();
    let mut updates: Vec<(String, ProductKey, ProductRecordUpdate)> = Vec::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Domain(domain_record) => {
                handle_domain_event(
                    &message_id,
                    domain_record,
                    &mut creates,
                    &mut updates,
                    &mut failed_message_ids,
                );
            }
            ProductEventRecord::Enrichment(enrichment_record) => {
                let key = enrichment_record.key();
                let update = build_enrichment_update(enrichment_record);
                updates.push((message_id, key, update));
            }
            ProductEventRecord::Policy(policy_record) => {
                let key = policy_record.key();
                let record_res = product_repository
                    .get_product_record(&policy_record.shop_id, &policy_record.shops_product_id)
                    .await;
                match record_res {
                    Ok(Some(record)) => {
                        let mut update_record = ProductRecordUpdate::default();
                        let prohibited_images = record
                            .images
                            .into_iter()
                            .map(|mut image| {
                                image.prohibited_content =
                                    policy_record.prohibited_content_decision;
                                image
                            })
                            .collect();
                        update_record.images = Some(prohibited_images);
                        updates.push((message_id, key, update_record));
                    }
                    Ok(None) => {
                        error!(
                            shopId = %policy_record.shop_id,
                            shopsProductId = %policy_record.shops_product_id,
                            "ProductRecord doesn't exist. This is a logic error. Impossible to apply policy to non-existent product."
                        );
                        failed_message_ids.push(message_id);
                    }
                    Err(err) => {
                        warn!(
                            error = ?err,
                            shopId = %policy_record.shop_id,
                            "Failed getting ProductRecord."
                        );
                        failed_message_ids.push(message_id);
                    }
                }
            }
        }
    }

    persist_creates(product_repository, creates, &mut failed_message_ids).await;
    persist_updates(product_repository, updates, &mut failed_message_ids).await;

    let failures = failed_message_ids.len();
    info!(
        successful = count - failures,
        failures = failures,
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

fn handle_domain_event(
    message_id: &str,
    domain_record: ProductDomainEventRecord,
    creates: &mut HashMap<String, ProductRecord>,
    updates: &mut Vec<(String, ProductKey, ProductRecordUpdate)>,
    failed_message_ids: &mut Vec<String>,
) {
    if domain_record.event_type == ProductDomainEventTypeRecord::DomainCreated {
        match ProductRecord::try_from(domain_record) {
            Ok(record) => {
                creates.insert(message_id.to_string(), record);
            }
            Err(err) => {
                error!(
                    error = %err,
                    fromType = %std::any::type_name::<ProductDomainEventRecord>(),
                    toType = %std::any::type_name::<ProductRecord>(),
                    "Failed mapping types."
                );
                failed_message_ids.push(message_id.to_string());
            }
        }
    } else {
        let key = domain_record.key();
        let update = ProductRecordUpdate::from(domain_record);
        updates.push((message_id.to_string(), key, update));
    }
}

fn build_enrichment_update(
    event_record: product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord,
) -> ProductRecordUpdate {
    ProductRecordUpdate::from(event_record)
}

async fn persist_creates(
    repository: &impl ProductDynamoDbRepository,
    creates: HashMap<String, ProductRecord>,
    failed_message_ids: &mut Vec<String>,
) {
    for batch in Batch::chunked_from(creates.into_iter()) {
        let batch: Batch<_, 25> = batch;
        let batch_message_ids = batch
            .iter()
            .map(|(message_id, record)| (record.key(), message_id.clone()))
            .collect::<HashMap<_, _>>();
        let batch = Batch::try_from_iter(batch.into_iter().map(|(_, record)| record))
            .expect("shouldn't fail re-building batch of same size from former batch");
        match repository.put_product_records(batch).await {
            Ok(output) => {
                let mut failures = Vec::new();
                handle_dynamodb_batch_write_put_product_output::<ProductRecord>(
                    output,
                    &mut failures,
                );
                for key in failures {
                    match batch_message_ids.get(&key) {
                        Some(message_id) => failed_message_ids.push(message_id.clone()),
                        None => {
                            error!(
                                productKey = %key,
                                "There exists no message_id for failed ProductRecord."
                            );
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire batch.");
                failed_message_ids.extend(batch_message_ids.into_values());
            }
        }
    }
}

async fn persist_updates(
    repository: &impl ProductDynamoDbRepository,
    updates: Vec<(String, ProductKey, ProductRecordUpdate)>,
    failed_message_ids: &mut Vec<String>,
) {
    for (message_id, key, update) in updates {
        let update_res = repository
            .update_product_record(&key.shop_id, &key.shops_product_id, update)
            .await;
        if let Err(err) = update_res {
            error!(error = ?err, productKey = %key, "Failed update.");
            failed_message_ids.push(message_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use aws_sdk_dynamodb::operation::update_item::UpdateItemOutput;
    use common::event::Event;
    use common::has_key::HasKey;
    use common::product_id::ProductKey;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::ProductEvent;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;
    use test_api::mk_partial_put_batch_failure;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn mk_event_bridge_payload(product_event_record: &impl serde::Serialize) -> String {
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

    fn mk_sqs_message(record: &impl serde::Serialize) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Faker.fake());
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    fn mk_sqs_message_with_id(record: &impl serde::Serialize, message_id: String) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(record));
        msg
    }

    // ---- Tests for DOMAIN_CREATED events (creates) ----

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
    async fn should_return_no_failures_when_all_created_events_processed(
        #[case] record_count: usize,
    ) {
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
            .map(|record| mk_sqs_message(&record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository.expect_put_product_records().returning(move |_| {
            Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
        });
        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
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
    async fn should_return_all_failures_when_ddb_batch_write_fails_for_created_events(
        #[case] record_count: usize,
    ) {
        let mut expected_message_ids = Vec::with_capacity(record_count);
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
            .map(|record| {
                let message_id = Uuid::new_v4().to_string();
                expected_message_ids.push(message_id.clone());
                mk_sqs_message_with_id(&record, message_id)
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let failed_keys: Arc<Mutex<Vec<ProductKey>>> = Arc::new(Mutex::new(vec![]));
        let failed_keys_clone = failed_keys.clone();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_put_product_records()
            .returning(move |batch| {
                batch
                    .into_iter()
                    .map(|record| record.key())
                    .for_each(|key| failed_keys_clone.lock().unwrap().push(key));
                Box::pin(async move { Err(SdkError::construction_failure("Something went wrong")) })
            });
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();
        expected_message_ids.sort();

        assert_eq!(expected_message_ids, actual_failed_message_ids);
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
    async fn should_return_partial_failures_when_ddb_returns_unprocessed_items_for_created_events(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
        let records: Vec<SqsMessage> = fake::vec![ProductCreatedDomainEventPayload; record_count]
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
            .map(|record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(record.key(), uuid.clone());
                mk_sqs_message_with_id(&record, uuid)
            })
            .collect();

        let expected_failures: Vec<ProductKey> =
            message_ids.keys().take(failure_count).cloned().collect();
        let expected_failures_clone = expected_failures.clone();

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_put_product_records()
            .returning(move |batch| {
                let unprocessed = batch
                    .into_iter()
                    .filter(|product_record| {
                        expected_failures_clone.contains(&product_record.key())
                    })
                    .collect();
                Box::pin(async move {
                    Ok(BatchWriteItemOutput::builder()
                        .set_unprocessed_items(mk_partial_put_batch_failure("table_1", unprocessed))
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
        let mut expected_failed_message_ids = expected_failures
            .into_iter()
            .filter_map(|key| message_ids.remove(&key))
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    // ---- Tests for update events (Domain non-create, Enrichment, Policy) ----

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
    async fn should_return_no_failures_when_all_update_events_processed(
        #[case] record_count: usize,
    ) {
        let records = fake::vec![ProductEvent; record_count]
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| mk_sqs_message(&event_record))
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_records()
            .returning(move |_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
        product_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        product_repository
            .expect_update_product_record()
            .returning(move |_, _, _| {
                Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
            });
        let actual = handler(&product_repository, lambda_event).await.unwrap();
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
    #[trace]
    async fn should_return_partial_failures_when_update_fails(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let events = fake::vec![ProductEvent; record_count];
        let expected_failed_events = events
            .clone()
            .into_iter()
            .take(failure_count)
            .collect::<Vec<_>>();
        let expected_failed_events_for_create = expected_failed_events.clone();
        let mut expected_failed_message_ids = Vec::with_capacity(failure_count);
        let records = events
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let message_id = Uuid::new_v4().to_string();
                if expected_failed_events
                    .iter()
                    .any(|event| event.payload.key() == event_record.key())
                {
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
        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_records()
            .returning(move |batch| {
                let expected_failure_keys: Vec<_> = expected_failed_events_for_create
                    .iter()
                    .map(|e| e.payload.key())
                    .collect();
                let unprocessed = batch
                    .into_iter()
                    .filter(|record| expected_failure_keys.contains(&record.key()))
                    .collect();
                Box::pin(async move {
                    Ok(BatchWriteItemOutput::builder()
                        .set_unprocessed_items(mk_partial_put_batch_failure("table_1", unprocessed))
                        .build())
                })
            });
        product_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        product_repository.expect_update_product_record().returning(
            move |shop_id, shops_product_id, _| {
                if expected_failed_events.iter().any(|event| {
                    &event.payload.key().shop_id == shop_id
                        && &event.payload.key().shops_product_id == shops_product_id
                }) {
                    Box::pin(
                        async move { Err(SdkError::construction_failure("Something went wrong.")) },
                    )
                } else {
                    Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
                }
            },
        );
        expected_failed_message_ids.sort();
        let mut actual_failed_message_ids = handler(&product_repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    // ---- Tests for mixed events (creates + updates in same batch) ----

    #[tokio::test]
    async fn should_return_no_failures_when_mixed_create_and_update_events_all_succeed() {
        let create_records: Vec<SqsMessage> = fake::vec![ProductCreatedDomainEventPayload; 5]
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
            .map(|record| mk_sqs_message(&record))
            .collect();
        let update_records: Vec<SqsMessage> = fake::vec![ProductEvent; 5]
            .into_iter()
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| mk_sqs_message(&record))
            .collect();

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = [create_records, update_records].concat();
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_records()
            .returning(move |_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
        product_repository
            .expect_get_product_record()
            .returning(move |_, _| Box::pin(async move { Ok(Some(Faker.fake())) }));
        product_repository
            .expect_update_product_record()
            .returning(move |_, _, _| {
                Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
            });
        let actual = handler(&product_repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_empty_batch() {
        let sqs_event = SqsEvent::default();
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let repository = MockProductDynamoDbRepository::default();
        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(Uuid::new_v4().to_string());
        msg.body = None;

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let repository = MockProductDynamoDbRepository::default();
        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let message_id = Uuid::new_v4().to_string();
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id.clone());
        msg.body = Some("invalid json {".to_string());

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = vec![msg];
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };
        let repository = MockProductDynamoDbRepository::default();
        let actual = handler(&repository, lambda_event).await.unwrap();
        assert_eq!(1, actual.batch_item_failures.len());
        assert_eq!(message_id, actual.batch_item_failures[0].item_identifier);
    }
}
