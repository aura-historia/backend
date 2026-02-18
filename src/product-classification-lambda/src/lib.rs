use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::batch::Batch;
use common::dynamodb_stream::extract_from_dynamodb_stream;
use common::event_id::EventId;
use lambda_runtime::LambdaEvent;
use product::core::product_event::enrichment::{
    ClassifiedCategoryProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
};
use product::core::product_event::{ProductEvent, ProductEventPayload};
use product::dynamodb::product_event_record::ProductEventRecord;
use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, extract_product_id};
use product_classification::category::service::CategoryService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info};

#[tracing::instrument(
    skip(
        product_repository,
        category_service,
        event
    ),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    product_repository: &impl ProductDynamoDbRepository,
    category_service: &impl CategoryService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");
    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEnrichmentEventRecord>(event.payload.records);

    let mut classification_event_records = HashMap::with_capacity(event_records.len());
    for (message_id, event_record) in event_records {
        let classification_event_record_res =
            classify_category(category_service, &message_id, &event_record).await;
        match classification_event_record_res {
            Ok(classification_event_record) => {
                classification_event_records.insert(message_id, classification_event_record);
            }
            Err(message_id) => failed_message_ids.push(message_id),
        }
    }

    for batch in Batch::chunked_from(classification_event_records.into_iter()) {
        let batch: Batch<_, 25> = batch;
        let batch_message_ids = batch
            .iter()
            .map(|(message_id, event_record)| (*event_record.product_id(), message_id.clone()))
            .collect::<HashMap<_, _>>();
        let batch = Batch::try_from_iter(batch.into_iter().map(|(_, event_record)| event_record))
            .expect("shouldn't fail re-building batch of same size from former batch");
        let batch_res = product_repository.put_product_event_records(batch).await;
        match batch_res {
            Ok(batch_output) => {
                batch_output
                    .unprocessed_items
                    .unwrap_or_default()
                    .into_iter()
                    .flat_map(|(_table, reqs)| reqs)
                    .map(|req| req.put_request.expect("shouldn't be any other request than 'PutRequest' because events are append-only").item)
                    .map(extract_product_id)
                    .filter_map(|result| match result {
                        Ok(product_id) => Some(product_id),
                        Err(err) => {
                            error!(error = %err, "Failed extracting ProductId.");
                            None
                        }
                    }).for_each(|product_id| match batch_message_ids.get(&product_id) {
                        Some(message_id) => {
                            failed_message_ids.push(message_id.clone());
                        },
                        None => {
                            error!("Failed finding messageId for failed ProductId.");
                        },
                    });
            }
            Err(err) => {
                error!(error = ?err, "Failed writing ProductEnrichmentClassifyCategoryEventRecord to DynamoDB.");
                failed_message_ids.extend(batch_message_ids.into_values());
            }
        }
    }

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

#[tracing::instrument(
    skip(
        category_service,
        message_id,
        event_record
    ),
    fields(
        messageId = message_id,
        productId = %event_record.product_id,
        shopId = %event_record.shop_id,
        shopsProductId = %event_record.shops_product_id,
    )
)]
async fn classify_category(
    category_service: &impl CategoryService,
    message_id: &str,
    event_record: &ProductEnrichmentEventRecord,
) -> Result<ProductEventRecord, String> {
    let embedding = event_record.text_embedding.as_ref().ok_or_else(|| {
        error!("Cannot find category for product when embedding is missing.");
        message_id
    })?;
    let (category, score) = category_service
        .find_similar(embedding, 1)
        .await
        .map_err(|err| {
            error!(error = %err, "Failed finding categories similar to product.");
            message_id
        })?
        .into_iter()
        .next()
        .ok_or_else(|| {
            error!("No categories were returned.");
            message_id
        })?;
    info!(
        categoryId = %category.category_id,
        score = score,
        "Classified product into category."
    );
    let classification_event = ProductEvent {
        aggregate_id: event_record.product_id,
        event_id: EventId::new(),
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::ClassifiedCategory(
                ClassifiedCategoryProductEnrichmentEventPayload {
                    shop_id: event_record.shop_id,
                    shops_product_id: event_record.shops_product_id.clone(),
                    category_id: category.category_id,
                },
            ),
        ),
    };
    let classification_event_record =
        ProductEventRecord::try_from(classification_event).map_err(|err| {
            error!(
                error = %err,
                fromType = %std::any::type_name::<ProductEvent>(),
                toType = %std::any::type_name::<ProductEventRecord>(),
                "Failed mapping",
            );
            message_id
        })?;

    Ok(classification_event_record)
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use aws_sdk_dynamodb::types::{PutRequest, WriteRequest};
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::enrichment::{
        EmbeddedTextProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
    };
    use product::core::product_event::{ProductEvent, ProductEventPayload};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use std::collections::{HashMap, HashSet};
    use std::time::SystemTime;
    use time::OffsetDateTime;
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

    fn mk_sqs_message_with_id(
        product_event_record: &ProductEventRecord,
        message_id: String,
    ) -> SqsMessage {
        let mut msg = SqsMessage::default();
        msg.message_id = Some(message_id);
        msg.body = Some(mk_event_bridge_payload(product_event_record));
        msg
    }

    fn mk_partial_put_batch_failure(
        table_name: &str,
        failures: Vec<ProductEventRecord>,
    ) -> Option<HashMap<String, Vec<WriteRequest>>> {
        let put_failures = failures
            .into_iter()
            .map(serde_dynamo::to_item)
            .map(Result::unwrap)
            .map(|ddb_item| {
                PutRequest::builder()
                    .set_item(Some(ddb_item))
                    .build()
                    .expect("should build PutRequest for unprocessed item")
            })
            .map(|put_req| {
                WriteRequest::builder()
                    .set_put_request(Some(put_req))
                    .build()
            })
            .collect();
        Some(HashMap::from([(table_name.to_string(), put_failures)]))
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
    #[trace]
    async fn should_return_no_failures_when_all_records_processed_for_classification(
        #[case] record_count: usize,
    ) {
        let records = fake::vec![EmbeddedTextProductEnrichmentEventPayload; record_count]
            .into_iter()
            .map(|payload| ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::EmbeddedText(payload),
                ),
            })
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
        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_event_records()
            .returning(move |_| {
                Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
            });
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(fake::vec![(Category, f64); 1]) }));

        let actual = handler(&product_repository, &category_service, lambda_event)
            .await
            .unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(25)]
    #[case(47)]
    #[trace]
    async fn should_return_all_failures_when_category_service_returns_no_categories_for_classification(
        #[case] record_count: usize,
    ) {
        let mut expected_message_ids = Vec::with_capacity(record_count);
        let records = fake::vec![EmbeddedTextProductEnrichmentEventPayload; record_count]
            .into_iter()
            .map(|payload| ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::EmbeddedText(payload),
                ),
            })
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|product_event_record| {
                let message_id = Uuid::new_v4().to_string();
                expected_message_ids.push(message_id.clone());
                mk_sqs_message_with_id(&product_event_record, message_id)
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
            .expect_put_product_event_records()
            .times(0);
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(vec![]) }));

        let mut actual_failed_message_ids =
            handler(&product_repository, &category_service, lambda_event)
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
    #[case(1, 2)]
    #[case(2, 5)]
    #[case(5, 25)]
    #[case(10, 47)]
    #[trace]
    async fn should_return_partial_failures_when_embeddings_missing_for_classification(
        #[case] missing_embedding_count: usize,
        #[case] record_count: usize,
    ) {
        let mut expected_message_ids = Vec::with_capacity(missing_embedding_count);
        let mut records = Vec::with_capacity(record_count);
        let success_count = record_count.saturating_sub(missing_embedding_count);

        for idx in 0..record_count {
            let payload: EmbeddedTextProductEnrichmentEventPayload = Faker.fake();
            let event = ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::EmbeddedText(payload),
                ),
            };
            let record = ProductEventRecord::try_from(event).unwrap();
            let message_id = Uuid::new_v4().to_string();

            let record = if idx < missing_embedding_count {
                let record = match record {
                    ProductEventRecord::Enrichment(mut enrichment_record) => {
                        enrichment_record.text_embedding = None;
                        ProductEventRecord::Enrichment(enrichment_record)
                    }
                    _ => unreachable!("expected enrichment event record"),
                };
                expected_message_ids.push(message_id.clone());
                record
            } else {
                record
            };

            records.push(mk_sqs_message_with_id(&record, message_id));
        }

        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let mut product_repository = MockProductDynamoDbRepository::default();
        if success_count == 0 {
            product_repository
                .expect_put_product_event_records()
                .times(0);
        } else {
            product_repository
                .expect_put_product_event_records()
                .returning(move |_| {
                    Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
                });
        }

        let mut category_service = MockCategoryService::default();
        if success_count == 0 {
            category_service.expect_find_similar().times(0);
        } else {
            category_service
                .expect_find_similar()
                .times(success_count)
                .returning(|_, _| Box::pin(async { Ok(fake::vec![(Category, f64); 1]) }));
        }

        let mut actual_failed_message_ids =
            handler(&product_repository, &category_service, lambda_event)
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
    #[case(1, 5)]
    #[case(5, 25)]
    #[case(10, 47)]
    #[case(20, 100)]
    #[trace]
    async fn should_return_partial_failures_when_ddb_returns_unprocessed_items_for_classification(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
        let mut product_ids = Vec::with_capacity(record_count);
        let records = fake::vec![EmbeddedTextProductEnrichmentEventPayload; record_count]
            .into_iter()
            .map(|payload| ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::EmbeddedText(payload),
                ),
            })
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|product_event_record| {
                let product_id = *product_event_record.product_id();
                let message_id = Uuid::new_v4().to_string();
                message_ids.insert(product_id, message_id.clone());
                product_ids.push(product_id);
                mk_sqs_message_with_id(&product_event_record, message_id)
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        let lambda_event = LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        };

        let expected_failure_product_ids: HashSet<_> =
            product_ids.iter().take(failure_count).cloned().collect();
        let expected_failure_product_ids_clone = expected_failure_product_ids.clone();

        let mut product_repository = MockProductDynamoDbRepository::default();
        product_repository
            .expect_put_product_event_records()
            .returning(move |batch| {
                let unprocessed = batch
                    .into_iter()
                    .filter(|record| {
                        expected_failure_product_ids_clone.contains(record.product_id())
                    })
                    .collect();
                Box::pin(async move {
                    Ok(BatchWriteItemOutput::builder()
                        .set_unprocessed_items(mk_partial_put_batch_failure("table_1", unprocessed))
                        .build())
                })
            });

        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(fake::vec![(Category, f64); 1]) }));

        let mut actual_failed_message_ids =
            handler(&product_repository, &category_service, lambda_event)
                .await
                .unwrap()
                .batch_item_failures
                .into_iter()
                .map(|failure| failure.item_identifier)
                .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        let mut expected_failed_message_ids = expected_failure_product_ids
            .into_iter()
            .filter_map(|product_id| message_ids.get(&product_id).cloned())
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(25)]
    #[case(47)]
    #[trace]
    async fn should_return_all_failures_when_ddb_write_fails_for_classification(
        #[case] record_count: usize,
    ) {
        let mut expected_message_ids = Vec::with_capacity(record_count);
        let records = fake::vec![EmbeddedTextProductEnrichmentEventPayload; record_count]
            .into_iter()
            .map(|payload| ProductEvent {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::EmbeddedText(payload),
                ),
            })
            .map(ProductEventRecord::try_from)
            .map(Result::unwrap)
            .map(|product_event_record| {
                let message_id = Uuid::new_v4().to_string();
                expected_message_ids.push(message_id.clone());
                mk_sqs_message_with_id(&product_event_record, message_id)
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
            .expect_put_product_event_records()
            .returning(move |_| {
                Box::pin(async { Err(SdkError::construction_failure("Something went wrong")) })
            });
        let mut category_service = MockCategoryService::default();
        category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(fake::vec![(Category, f64); 1]) }));

        let mut actual_failed_message_ids =
            handler(&product_repository, &category_service, lambda_event)
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
}
