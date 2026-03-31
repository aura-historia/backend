pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    batch::{Batch, dynamodb::handle_dynamodb_batch_write_put_product_output},
    category_key::CategoryId,
    dynamodb_stream::extract_from_dynamodb_stream,
    event_id::EventId,
    has_key::HasKey,
    period_key::PeriodId,
    product_id::ProductKey,
};
use lambda_runtime::LambdaEvent;
use product::{
    core::{
        product_event::{
            ProductEvent, ProductEventPayload,
            enrichment::{
                ClassifiedCategoryProductEnrichmentEventPayload,
                ClassifiedPeriodProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
            },
        },
        title::Title,
    },
    dynamodb::{product_event_record::ProductEventRecord, repository::ProductDynamoDbRepository},
};
use product_classification::{category::service::CategoryService, period::service::PeriodService};
use service::ClassificationService;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::{error, info};

#[tracing::instrument(
    skip(classification_service, product_repository, category_service, period_service, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    classification_service: &(impl ClassificationService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    category_service: &(impl CategoryService + Sync),
    period_service: &(impl PeriodService + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut enrichment_events: Vec<(String, ProductEventRecord)> = Vec::new();

    for (message_id, event_record) in event_records {
        let enrichment_record = match event_record {
            ProductEventRecord::Enrichment(enrichment_record) => enrichment_record,
            other => {
                let key = other.key();
                error!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    eventId = %other.event_id(),
                    "Unexpected non-Enrichment event record type in classify handler."
                );
                continue;
            }
        };

        let embedding = match enrichment_record.embedding.as_ref() {
            Some(embedding) => embedding,
            None => {
                error!(
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Enrichment record has no embedding."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let product_record = match product_repository
            .get_product_record(
                &enrichment_record.shop_id,
                &enrichment_record.shops_product_id,
            )
            .await
        {
            Ok(Some(record)) => record,
            Ok(None) => {
                error!(
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Product record not found in DynamoDB."
                );
                failed_message_ids.push(message_id);
                continue;
            }
            Err(err) => {
                error!(
                    error = %err,
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Failed reading product record from DynamoDB."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let title = Title::from(product_record.title_native.text.as_str());

        let (similar_categories_res, similar_periods_res) = tokio::join!(
            category_service.find_similar(embedding, 5),
            period_service.find_similar(embedding, 5),
        );

        let category_ids: Vec<CategoryId> = match similar_categories_res {
            Ok(categories) => {
                if categories.is_empty() {
                    error!(
                        messageId = message_id,
                        shopId = %enrichment_record.shop_id,
                        shopsProductId = %enrichment_record.shops_product_id,
                        "No similar categories found."
                    );
                    failed_message_ids.push(message_id);
                    continue;
                }
                categories
                    .iter()
                    .map(|(c, _)| c.category_id.clone())
                    .collect()
            }
            Err(err) => {
                error!(
                    error = ?err,
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Failed finding similar categories."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let period_ids: Vec<PeriodId> = match similar_periods_res {
            Ok(periods) => {
                if periods.is_empty() {
                    error!(
                        messageId = message_id,
                        shopId = %enrichment_record.shop_id,
                        shopsProductId = %enrichment_record.shops_product_id,
                        "No similar periods found."
                    );
                    failed_message_ids.push(message_id);
                    continue;
                }
                periods.iter().map(|(p, _)| p.period_id.clone()).collect()
            }
            Err(err) => {
                error!(
                    error = ?err,
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Failed finding similar periods."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let (chosen_category, chosen_period) = match classification_service
            .classify(&title, &category_ids, &period_ids)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                error!(
                    error = %err,
                    messageId = message_id,
                    shopId = %enrichment_record.shop_id,
                    shopsProductId = %enrichment_record.shops_product_id,
                    "Failed classifying product."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let now = OffsetDateTime::now_utc();
        let product_id = enrichment_record.product_id;

        let category_event = ProductEvent {
            aggregate_id: product_id,
            event_id: EventId::new(),
            timestamp: now,
            payload: ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::ClassifiedCategory(
                    ClassifiedCategoryProductEnrichmentEventPayload {
                        shop_id: enrichment_record.shop_id,
                        shops_product_id: enrichment_record.shops_product_id.clone(),
                        category_id: chosen_category,
                    },
                ),
            ),
        };

        let period_event = ProductEvent {
            aggregate_id: product_id,
            event_id: EventId::new(),
            timestamp: now,
            payload: ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::ClassifiedPeriod(
                    ClassifiedPeriodProductEnrichmentEventPayload {
                        shop_id: enrichment_record.shop_id,
                        shops_product_id: enrichment_record.shops_product_id,
                        period_id: chosen_period,
                    },
                ),
            ),
        };

        let category_record: ProductEventRecord = category_event.into();
        let period_record: ProductEventRecord = period_event.into();
        enrichment_events.push((message_id.clone(), category_record));
        enrichment_events.push((message_id, period_record));
    }

    persist_enrichment_events(
        product_repository,
        enrichment_events,
        &mut failed_message_ids,
    )
    .await;

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

async fn persist_enrichment_events(
    repository: &(impl ProductDynamoDbRepository + Sync),
    enrichment_events: Vec<(String, ProductEventRecord)>,
    failed_message_ids: &mut Vec<String>,
) {
    for batch in Batch::chunked_from(enrichment_events.into_iter()) {
        let batch: Batch<_, 25> = batch;
        let batch_message_ids = batch
            .iter()
            .map(|(message_id, record)| (record.key(), message_id.clone()))
            .collect::<HashMap<ProductKey, String>>();
        let batch = Batch::try_from_iter(batch.into_iter().map(|(_, record)| record))
            .expect("shouldn't fail re-building batch of same size from former batch");
        match repository.put_product_event_records(batch).await {
            Ok(output) => {
                let mut failures = Vec::new();
                handle_dynamodb_batch_write_put_product_output::<ProductEventRecord>(
                    output,
                    &mut failures,
                );
                for key in failures {
                    match batch_message_ids.get(&key) {
                        Some(message_id) => failed_message_ids.push(message_id.clone()),
                        None => {
                            error!(
                                productKey = %key,
                                "There exists no message_id for failed ProductEventRecord."
                            );
                        }
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed entire enrichment event batch.");
                failed_message_ids.extend(batch_message_ids.into_values());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use common::event_id::EventId;
    use common::product_id::ProductId;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product_classification::category::core::Category;
    use product_classification::category::service::MockCategoryService;
    use product_classification::period::core::Period;
    use product_classification::period::service::MockPeriodService;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::service::{ClassificationError, MockClassificationService};

    fn mk_event_bridge_payload(event_record: &impl serde::Serialize) -> String {
        let mut stream_record = StreamRecord::default();
        stream_record.approximate_creation_date_time = SystemTime::now().into();
        stream_record.new_image = serde_dynamo::to_item(event_record).unwrap();
        stream_record.size_bytes = 42;

        let mut event = EventRecord::default();
        event.aws_region = "eu-central-1".to_string();
        event.change = stream_record;
        event.event_id = Uuid::new_v4().to_string();
        event.event_name = "INSERT".to_string();

        let mut eb_event = EventBridgeEvent::<EventRecord>::default();
        eb_event.detail_type = "DynamoDBStreamRecord".to_string();
        eb_event.source = "test-table".to_string();
        eb_event.detail = event;

        serde_json::to_string(&eb_event).unwrap()
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

    fn mk_lambda_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = messages;
        LambdaEvent {
            payload: sqs_event,
            context: Context::default(),
        }
    }

    fn mk_enrichment_event_record() -> ProductEnrichmentEventRecord {
        let mut record: ProductEnrichmentEventRecord = Faker.fake();
        record.embedding = Some(vec![0.42f32; 768]);
        record
    }

    fn mk_domain_event_record() -> ProductDomainEventRecord {
        let payload: ProductCreatedDomainEventPayload = Faker.fake();
        let event = Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(payload),
        };
        event.into()
    }

    fn mk_product_record() -> ProductRecord {
        ProductRecord::try_from(mk_domain_event_record()).unwrap()
    }

    fn mk_category_service_returning_categories() -> MockCategoryService {
        let mut mock = MockCategoryService::default();
        mock.expect_find_similar().returning(|_, _| {
            let category: Category = Faker.fake();
            Box::pin(async move { Ok(vec![(category, 0.95)]) })
        });
        mock
    }

    fn mk_period_service_returning_periods() -> MockPeriodService {
        let mut mock = MockPeriodService::default();
        mock.expect_find_similar().returning(|_, _| {
            let period: Period = Faker.fake();
            Box::pin(async move { Ok(vec![(period, 0.9)]) })
        });
        mock
    }

    fn mk_classification_service_returning_first() -> MockClassificationService {
        let mut mock = MockClassificationService::default();
        mock.expect_classify().returning(|_, categories, periods| {
            let cat = categories[0].clone();
            let per = periods[0].clone();
            Box::pin(async move { Ok((cat, per)) })
        });
        mock
    }

    fn mk_repository_returning_record(
        product_record: ProductRecord,
    ) -> MockProductDynamoDbRepository {
        let mut mock = MockProductDynamoDbRepository::default();
        mock.expect_get_product_record().returning(move |_, _| {
            let record = product_record.clone();
            Box::pin(async move { Ok(Some(record)) })
        });
        mock.expect_put_product_event_records().returning(|_| {
            Box::pin(async {
                Ok(
                    aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build(),
                )
            })
        });
        mock
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_classification_service = MockClassificationService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let mock_category_service = MockCategoryService::default();
        let mock_period_service = MockPeriodService::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_classified_successfully() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);

        let mock_classification_service = mk_classification_service_returning_first();
        let mock_repository = mk_repository_returning_record(mk_product_record());
        let mock_category_service = mk_category_service_returning_categories();
        let mock_period_service = mk_period_service_returning_periods();

        let event = mk_lambda_event(vec![mk_sqs_message(&event_record)]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_classification_fails() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-classify-fail".to_string();

        let mut mock_classification_service = MockClassificationService::default();
        mock_classification_service
            .expect_classify()
            .times(1)
            .returning(|_, _, _| {
                Box::pin(async {
                    Err(ClassificationError::InvalidResponse(
                        "bad response".to_string(),
                    ))
                })
            });

        let mock_repository = {
            let product_record = mk_product_record();
            let mut mock = MockProductDynamoDbRepository::default();
            mock.expect_get_product_record().returning(move |_, _| {
                let record = product_record.clone();
                Box::pin(async move { Ok(Some(record)) })
            });
            mock
        };
        let mock_category_service = mk_category_service_returning_categories();
        let mock_period_service = mk_period_service_returning_periods();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_product_record_not_found() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-not-found".to_string();

        let mock_classification_service = MockClassificationService::default();
        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_get_product_record()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        let mock_category_service = MockCategoryService::default();
        let mock_period_service = MockPeriodService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_enrichment_record_has_no_embedding() {
        let mut enrichment_record: ProductEnrichmentEventRecord = Faker.fake();
        enrichment_record.embedding = None;
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-no-embedding".to_string();

        let mock_classification_service = MockClassificationService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let mock_category_service = MockCategoryService::default();
        let mock_period_service = MockPeriodService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_non_enrichment_records() {
        let domain_record = mk_domain_event_record();
        let event_record = ProductEventRecord::Domain(domain_record);
        let message_id = "non-enrichment-msg".to_string();

        let mock_classification_service = MockClassificationService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let mock_category_service = MockCategoryService::default();
        let mock_period_service = MockPeriodService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_no_similar_categories_found() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-no-categories".to_string();

        let mock_classification_service = MockClassificationService::default();
        let mock_repository = {
            let product_record = mk_product_record();
            let mut mock = MockProductDynamoDbRepository::default();
            mock.expect_get_product_record().returning(move |_, _| {
                let record = product_record.clone();
                Box::pin(async move { Ok(Some(record)) })
            });
            mock
        };
        let mut mock_category_service = MockCategoryService::default();
        mock_category_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(vec![]) }));
        let mock_period_service = mk_period_service_returning_periods();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_no_similar_periods_found() {
        let enrichment_record = mk_enrichment_event_record();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "test-msg-no-periods".to_string();

        let mock_classification_service = MockClassificationService::default();
        let mock_repository = {
            let product_record = mk_product_record();
            let mut mock = MockProductDynamoDbRepository::default();
            mock.expect_get_product_record().returning(move |_, _| {
                let record = product_record.clone();
                Box::pin(async move { Ok(Some(record)) })
            });
            mock
        };
        let mock_category_service = mk_category_service_returning_categories();
        let mut mock_period_service = MockPeriodService::default();
        mock_period_service
            .expect_find_similar()
            .returning(|_, _| Box::pin(async { Ok(vec![]) }));

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_products_classified_successfully() {
        let enrichment_record1 = mk_enrichment_event_record();
        let event_record1 = ProductEventRecord::Enrichment(enrichment_record1);
        let enrichment_record2 = mk_enrichment_event_record();
        let event_record2 = ProductEventRecord::Enrichment(enrichment_record2);

        let mut mock_classification_service = MockClassificationService::default();
        mock_classification_service
            .expect_classify()
            .times(2)
            .returning(|_, categories, periods| {
                let cat = categories[0].clone();
                let per = periods[0].clone();
                Box::pin(async move { Ok((cat, per)) })
            });

        let product_record = mk_product_record();
        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_get_product_record()
            .times(2)
            .returning(move |_, _| {
                let record = product_record.clone();
                Box::pin(async move { Ok(Some(record)) })
            });
        mock_repository
            .expect_put_product_event_records()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build())
                })
            });

        let mut mock_category_service = MockCategoryService::default();
        mock_category_service
            .expect_find_similar()
            .times(2)
            .returning(|_, _| {
                let category: Category = Faker.fake();
                Box::pin(async move { Ok(vec![(category, 0.95)]) })
            });

        let mut mock_period_service = MockPeriodService::default();
        mock_period_service
            .expect_find_similar()
            .times(2)
            .returning(|_, _| {
                let period: Period = Faker.fake();
                Box::pin(async move { Ok(vec![(period, 0.9)]) })
            });

        let event = mk_lambda_event(vec![
            mk_sqs_message(&event_record1),
            mk_sqs_message(&event_record2),
        ]);
        let result = handler(
            &mock_classification_service,
            &mock_repository,
            &mock_category_service,
            &mock_period_service,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
