pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    batch::{Batch, dynamodb::handle_dynamodb_batch_write_put_product_output},
    dynamodb_stream::extract_from_dynamodb_stream,
    has_key::HasKey,
    product_id::ProductKey,
};
use lambda_runtime::LambdaEvent;
use product::{
    core::{product::Product, product_event::ProductEventPayload},
    dynamodb::{
        product_event_record::ProductEventRecord, product_record::ProductRecord,
        repository::ProductDynamoDbRepository,
    },
};
use service::MultimodalEmbeddingService;
use std::collections::HashMap;
use tracing::{debug, error, info};

#[tracing::instrument(
    skip(embedding_service, product_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    embedding_service: &(impl MultimodalEmbeddingService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut enrichment_events: Vec<(String, ProductEventRecord)> = Vec::new();

    for (message_id, event_record) in event_records {
        let mut product: Product = match event_record {
            ProductEventRecord::Domain(domain_record) => {
                let key = domain_record.key();
                match ProductRecord::try_from(domain_record).map(Product::from) {
                    Ok(product) => product,
                    Err(err) => {
                        error!(
                            error = %err,
                            messageId = message_id,
                            shopId = %key.shop_id,
                            shopsProductId = %key.shops_product_id,
                            "Failed converting domain event record to Product."
                        );
                        continue;
                    }
                }
            }
            other => {
                let key = other.key();
                error!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    eventId = %other.event_id(),
                    "Unexpected non-Domain event record type in embed-text handler."
                );
                continue;
            }
        };

        let image_url = product.images.first().map(|img| &img.url);
        let embedding = match embedding_service
            .embed(
                &product.native_title.payload,
                product.native_description.as_ref().map(|d| &d.payload),
                image_url,
            )
            .await
        {
            Ok(embedding) => embedding,
            Err(err) => {
                error!(
                    error = %err,
                    messageId = message_id,
                    shopId = %product.shop_id,
                    shopsProductId = %product.shops_product_id,
                    eventId = %product.event_id,
                    "Failed generating embedding."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        if let Some(enrichment_event) = product.embed_text(embedding) {
            let product_event = enrichment_event.map_payload(ProductEventPayload::from);
            let event_record: ProductEventRecord = product_event.into();
            enrichment_events.push((message_id, event_record));
        } else {
            debug!(
                messageId = message_id,
                shopId = %product.shop_id,
                shopsProductId = %product.shops_product_id,
                "Embedding unchanged, skipping."
            );
        }
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
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use service::MockMultimodalEmbeddingService;
    use std::time::SystemTime;
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::service::{self, MultimodalEmbeddingError};

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

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_embedded_successfully() {
        let record = mk_domain_event_record();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.1, 0.2, 0.3]) }));

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build())
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_failure_when_domain_record_is_malformed() {
        let mut record = mk_domain_event_record();
        record.title_native = None;
        let message_id = "test-msg-1".to_string();

        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_embedding_fails() {
        let record = mk_domain_event_record();
        let message_id = "test-msg-2".to_string();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Err(MultimodalEmbeddingError::EmptyResponse) }));

        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    // NOTE: `should_skip_persist_when_embedding_unchanged` is no longer applicable.
    // The Product is always constructed from a DOMAIN_CREATED event record, which always
    // sets `embedding: None` (see `TryFrom<ProductDomainEventRecord> for ProductRecord`).
    // Therefore, `product.embed_text(embedding)` always produces an enrichment event
    // (the new embedding never equals the absent existing one), so the "unchanged" path
    // can never be triggered from this handler.

    #[tokio::test]
    async fn should_return_partial_failures_when_some_succeed_and_some_fail() {
        let record_success = mk_domain_event_record();
        let mut record_fail = mk_domain_event_record();
        record_fail.title_native = None;
        let success_msg_id = "success-msg".to_string();
        let fail_msg_id = "fail-msg".to_string();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.5, 0.6]) }));

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build())
                })
            });

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record_success, success_msg_id),
            mk_sqs_message_with_id(&record_fail, fail_msg_id.clone()),
        ]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let mut empty_msg = SqsMessage::default();
        empty_msg.message_id = Some("empty-body-msg".to_string());
        empty_msg.body = None;

        let event = mk_lambda_event(vec![empty_msg]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-json-msg".to_string());
        invalid_msg.body = Some("invalid json {".to_string());

        let event = mk_lambda_event(vec![invalid_msg]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            "invalid-json-msg",
            result.batch_item_failures[0].item_identifier
        );
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_products_embedded_successfully() {
        let record1 = mk_domain_event_record();
        let record2 = mk_domain_event_record();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.1, 0.2]) }));

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build())
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record1), mk_sqs_message(&record2)]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_failure_when_non_domain_event_record_received() {
        let enrichment_record: ProductEnrichmentEventRecord = Faker.fake();
        let event_record = ProductEventRecord::Enrichment(enrichment_record);
        let message_id = "non-domain-msg".to_string();

        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(&mock_embedding_service, &mock_repository, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
