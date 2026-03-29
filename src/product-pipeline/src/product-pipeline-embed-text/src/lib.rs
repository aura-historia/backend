pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{batch::Batch, dynamodb_stream::extract_from_dynamodb_stream, has_key::HasKey};
use lambda_runtime::LambdaEvent;
use product::{
    core::product_event::ProductEventPayload,
    dynamodb::{product_event_record::ProductEventRecord, repository::ProductDynamoDbRepository},
    service::get_service::GetProductService,
};
use service::MultimodalEmbeddingService;
use tracing::{debug, error, info};

#[tracing::instrument(
    skip(get_product_service, embedding_service, product_repository, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    get_product_service: &(impl GetProductService + Sync),
    embedding_service: &(impl MultimodalEmbeddingService + Sync),
    product_repository: &(impl ProductDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();
    info!(count = count, "Handler invoked.");

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    for (message_id, event_record) in event_records {
        let product_key = event_record.key();

        let product = match get_product_service
            .find_product(&product_key.shop_id, &product_key.shops_product_id)
            .await
        {
            Ok(product) => product,
            Err(err) => {
                error!(
                    error = %err,
                    messageId = message_id,
                    shopId = %product_key.shop_id,
                    shopsProductId = %product_key.shops_product_id,
                    "Failed fetching product."
                );
                failed_message_ids.push(message_id);
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
                    shopId = %product_key.shop_id,
                    shopsProductId = %product_key.shops_product_id,
                    "Failed generating embedding."
                );
                failed_message_ids.push(message_id);
                continue;
            }
        };

        let mut product = product;
        if let Some(enrichment_event) = product.embed_text(embedding) {
            let product_event = enrichment_event.map_payload(ProductEventPayload::from);
            let event_record: ProductEventRecord = product_event.into();
            let batch: Batch<ProductEventRecord, 25> = vec![event_record]
                .try_into()
                .expect("shouldn't fail creating batch of 1 item");

            if let Err(err) = product_repository.put_product_event_records(batch).await {
                error!(
                    error = ?err,
                    messageId = message_id,
                    shopId = %product_key.shop_id,
                    shopsProductId = %product_key.shops_product_id,
                    "Failed persisting enrichment event."
                );
                failed_message_ids.push(message_id);
                continue;
            }

            debug!(
                messageId = message_id,
                shopId = %product_key.shop_id,
                shopsProductId = %product_key.shops_product_id,
                "Embedded product."
            );
        } else {
            debug!(
                messageId = message_id,
                shopId = %product_key.shop_id,
                shopsProductId = %product_key.shops_product_id,
                "Embedding unchanged, skipping."
            );
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

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product::Product;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::repository::MockProductDynamoDbRepository;
    use product::service::get_service::{GetProductError, MockGetProductService};
    use service::MockMultimodalEmbeddingService;
    use std::time::SystemTime;
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
        Faker.fake::<ProductDomainEventRecord>()
    }

    fn mk_product_from_record(record: &ProductDomainEventRecord) -> Product {
        let mut product: Product = Faker.fake();
        product.shop_id = record.shop_id;
        product.shops_product_id = record.shops_product_id.clone();
        product.text_embedding = None;
        product
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_get_service = MockGetProductService::default();
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_embedded_successfully() {
        let record = mk_domain_event_record();
        let product = mk_product_from_record(&record);
        let shop_id = product.shop_id;
        let shops_product_id = product.shops_product_id.clone();

        let mut mock_get_service = MockGetProductService::default();
        let product_clone = product.clone();
        mock_get_service
            .expect_find_product()
            .withf(move |sid, spid| *sid == shop_id && *spid == shops_product_id)
            .times(1)
            .returning(move |_, _| {
                let p = product_clone.clone();
                Box::pin(async move { Ok(p) })
            });

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
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_product_not_found() {
        let record = mk_domain_event_record();
        let message_id = "test-msg-1".to_string();

        let mut mock_get_service = MockGetProductService::default();
        mock_get_service
            .expect_find_product()
            .times(1)
            .returning(|sid, spid| {
                let sid = *sid;
                let spid = spid.clone();
                Box::pin(async move { Err(GetProductError::ProductNotFound(sid, spid)) })
            });

        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_embedding_fails() {
        let record = mk_domain_event_record();
        let product = mk_product_from_record(&record);
        let message_id = "test-msg-2".to_string();

        let mut mock_get_service = MockGetProductService::default();
        let product_clone = product.clone();
        mock_get_service
            .expect_find_product()
            .times(1)
            .returning(move |_, _| {
                let p = product_clone.clone();
                Box::pin(async move { Ok(p) })
            });

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Err(MultimodalEmbeddingError::EmptyResponse) }));

        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_persist_when_embedding_unchanged() {
        let record = mk_domain_event_record();
        let mut product = mk_product_from_record(&record);
        let existing_embedding = vec![0.1, 0.2, 0.3];
        product.text_embedding = Some(existing_embedding.clone());

        let mut mock_get_service = MockGetProductService::default();
        let product_clone = product.clone();
        mock_get_service
            .expect_find_product()
            .times(1)
            .returning(move |_, _| {
                let p = product_clone.clone();
                Box::pin(async move { Ok(p) })
            });

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(move |_, _, _| {
                let e = existing_embedding.clone();
                Box::pin(async move { Ok(e) })
            });

        // Repository should NOT be called since embedding is unchanged
        let mock_repository = MockProductDynamoDbRepository::default();

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_partial_failures_when_some_succeed_and_some_fail() {
        let record_success = mk_domain_event_record();
        let record_fail = mk_domain_event_record();
        let product_success = mk_product_from_record(&record_success);
        let product_fail = mk_product_from_record(&record_fail);
        let success_msg_id = "success-msg".to_string();
        let fail_msg_id = "fail-msg".to_string();

        let fail_shop_id = product_fail.shop_id;
        let fail_spid = product_fail.shops_product_id.clone();

        let mut mock_get_service = MockGetProductService::default();
        let ps = product_success.clone();
        let fail_spid_clone = fail_spid.clone();
        mock_get_service
            .expect_find_product()
            .withf(move |sid, spid| *sid != fail_shop_id || *spid != fail_spid_clone)
            .times(1)
            .returning(move |_, _| {
                let p = ps.clone();
                Box::pin(async move { Ok(p) })
            });
        mock_get_service
            .expect_find_product()
            .withf(move |sid, spid| *sid == fail_shop_id || *spid == fail_spid)
            .times(1)
            .returning(|sid, spid| {
                let sid = *sid;
                let spid = spid.clone();
                Box::pin(async move { Err(GetProductError::ProductNotFound(sid, spid)) })
            });

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
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(fail_msg_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mock_get_service = MockGetProductService::default();
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let mut empty_msg = SqsMessage::default();
        empty_msg.message_id = Some("empty-body-msg".to_string());
        empty_msg.body = None;

        let event = mk_lambda_event(vec![empty_msg]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let mock_get_service = MockGetProductService::default();
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_repository = MockProductDynamoDbRepository::default();

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-json-msg".to_string());
        invalid_msg.body = Some("invalid json {".to_string());

        let event = mk_lambda_event(vec![invalid_msg]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
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
        let product1 = mk_product_from_record(&record1);
        let product2 = mk_product_from_record(&record2);

        let mut mock_get_service = MockGetProductService::default();
        let p1 = product1.clone();
        let p2 = product2.clone();
        let shop_id_1 = product1.shop_id;
        let spid_1 = product1.shops_product_id.clone();
        mock_get_service
            .expect_find_product()
            .withf(move |sid, spid| *sid == shop_id_1 && *spid == spid_1)
            .times(1)
            .returning(move |_, _| {
                let p = p1.clone();
                Box::pin(async move { Ok(p) })
            });
        let shop_id_2 = product2.shop_id;
        let spid_2 = product2.shops_product_id.clone();
        mock_get_service
            .expect_find_product()
            .withf(move |sid, spid| *sid == shop_id_2 && *spid == spid_2)
            .times(1)
            .returning(move |_, _| {
                let p = p2.clone();
                Box::pin(async move { Ok(p) })
            });

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.1, 0.2]) }));

        let mut mock_repository = MockProductDynamoDbRepository::default();
        mock_repository
            .expect_put_product_event_records()
            .times(2)
            .returning(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder()
                        .build())
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message(&record1), mk_sqs_message(&record2)]);
        let result = handler(
            &mock_get_service,
            &mock_embedding_service,
            &mock_repository,
            event,
        )
        .await
        .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }
}
