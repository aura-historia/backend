pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    dynamodb_stream::extract_from_dynamodb_stream,
    has_key::HasKey,
    logging::{LogEventType, LogPipelineStage},
    product_id::ProductKey,
};
use lambda_runtime::LambdaEvent;
use product::{
    dynamodb::product_event_record::ProductEventRecord,
    service::{command_service::CommandProductService, product_command::UpdateProductCommand},
};
use service::MultimodalEmbeddingService;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(embedding_service, command_service, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    embedding_service: &(impl MultimodalEmbeddingService + Sync),
    command_service: &(impl CommandProductService + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    let mut key_to_messages: HashMap<ProductKey, Vec<String>> = HashMap::new();
    let mut records_by_key: HashMap<
        ProductKey,
        Vec<product::dynamodb::product_event_record::domain::ProductDomainEventRecord>,
    > = HashMap::new();

    for (message_id, event_record) in event_records {
        match event_record {
            ProductEventRecord::Domain(ref domain_record) => {
                let key = ProductKey::new(
                    domain_record.shop_id,
                    domain_record.shops_product_id.clone(),
                );
                key_to_messages
                    .entry(key.clone())
                    .or_default()
                    .push(message_id);
                records_by_key
                    .entry(key)
                    .or_default()
                    .push(domain_record.clone());
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
            }
        }
    }

    if records_by_key.is_empty() {
        let failures = failed_message_ids.len();
        info!(
            eventType = %LogEventType::BatchProcessing,
            pipelineStage = %LogPipelineStage::ProductEmbedding,
            processed = count,
            successful = 0,
            failures = failures,
            "Processed product embedding batch."
        );
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    // Compute embeddings for each domain event record, collecting per-product commands.
    let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();

    for (key, domain_records) in &records_by_key {
        let Some(domain_record) = domain_records
            .iter()
            .rev()
            .find(|record| record.title_native.is_some())
            .or_else(|| domain_records.last())
        else {
            continue;
        };
        let primary_message_id = key_to_messages
            .get(key)
            .and_then(|message_ids| message_ids.last())
            .cloned()
            .unwrap_or_default();

        let title = match &domain_record.title_native {
            Some(t) => &t.text,
            None => {
                warn!(
                    messageId = %primary_message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "Domain event has no native title — skipping embedding."
                );
                continue;
            }
        };

        // Borrow the domain record fields needed for embedding.
        let title_obj = product::core::title::Title::from(title.as_str());
        let description = domain_record
            .description_native
            .as_ref()
            .map(|d| product::core::description::Description::from(d.text.as_str()));
        let image_url = domain_record
            .images
            .as_ref()
            .and_then(|imgs| imgs.first())
            .map(|img| &img.url);

        let embedding = match embedding_service
            .embed(&title_obj, description.as_ref(), image_url)
            .await
        {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    error = %err,
                    messageId = %primary_message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "Failed generating embedding — marking message as failed."
                );
                if let Some(message_ids) = key_to_messages.get(key) {
                    failed_message_ids.extend(message_ids.iter().cloned());
                }
                continue;
            }
        };

        update_cmds.insert(
            key.clone(),
            UpdateProductCommand {
                embedding: Some(embedding),
                ..UpdateProductCommand::default()
            },
        );
    }

    if !update_cmds.is_empty() {
        let failed_keys = command_service.update(update_cmds).await;
        for (key, _cmd) in failed_keys {
            if let Some(message_ids) = key_to_messages.get(&key) {
                failed_message_ids.extend(message_ids.iter().cloned());
            } else {
                error!(
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "No message_id found for failed embed-text update command."
                );
            }
        }
    }

    let failures = failed_message_ids.len();
    info!(
        eventType = %LogEventType::BatchProcessing,
        pipelineStage = %LogPipelineStage::ProductEmbedding,
        processed = count,
        successful = count - failures,
        failures = failures,
        "Processed product embedding batch."
    );
    let mut sqs_batch_response = SqsBatchResponse::default();
    sqs_batch_response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(mk_batch_item_failure)
        .collect();
    Ok(sqs_batch_response)
}

fn mk_batch_item_failure(item_identifier: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = item_identifier;
    failure
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use common::event_id::EventId;
    use common::has_key::HasKey;
    use common::product_id::ProductId;
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::service::command_service::MockCommandProductService;
    use product::service::product_command::UpdateProductCommand;
    use service::MockMultimodalEmbeddingService;
    use std::collections::HashMap;
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
        let mock_command_service = MockCommandProductService::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(&mock_embedding_service, &mock_command_service, event)
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

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_failure_when_domain_record_has_no_native_title() {
        let mut record = mk_domain_event_record();
        record.title_native = None;
        let message_id = "test-msg-1".to_string();

        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
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

        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_command_service_update_fails() {
        use common::product_id::ProductKey;
        let record = mk_domain_event_record();
        let key = ProductKey::new(record.shop_id, record.shops_product_id.clone());
        let message_id = "test-msg-update-fail".to_string();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.5]) }));

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(move |cmds| {
                Box::pin(async move {
                    // Return all commands as failures
                    cmds
                })
            });

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &ProductEventRecord::Domain(record),
            message_id.clone(),
        )]);
        // Suppress unused variable warning
        let _ = key;
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_partial_failures_when_some_succeed_and_some_fail() {
        let record_success = mk_domain_event_record();
        let mut record_no_title = mk_domain_event_record();
        record_no_title.title_native = None;
        let success_msg_id = "success-msg".to_string();
        let no_title_msg_id = "no-title-msg".to_string();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.5, 0.6]) }));

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record_success, success_msg_id),
            mk_sqs_message_with_id(&record_no_title, no_title_msg_id),
        ]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_messages_with_empty_body() {
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_command_service = MockCommandProductService::default();

        let mut empty_msg = SqsMessage::default();
        empty_msg.message_id = Some("empty-body-msg".to_string());
        empty_msg.body = None;

        let event = mk_lambda_event(vec![empty_msg]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let mock_embedding_service = MockMultimodalEmbeddingService::default();
        let mock_command_service = MockCommandProductService::default();

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-json-msg".to_string());
        invalid_msg.body = Some("invalid json {".to_string());

        let event = mk_lambda_event(vec![invalid_msg]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
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

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record1), mk_sqs_message(&record2)]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
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
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &event_record,
            message_id.clone(),
        )]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_set_embedding_field_in_update_command() {
        let record = mk_domain_event_record();
        let expected_embedding = vec![0.11f32, 0.22f32, 0.33f32];
        let expected_embedding_clone = expected_embedding.clone();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(move |_, _, _| {
                let v = expected_embedding_clone.clone();
                Box::pin(async move { Ok(v) })
            });

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .withf(move |cmds| {
                cmds.values()
                    .all(|cmd| cmd.embedding == Some(expected_embedding.clone()))
            })
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_keep_embeddable_domain_record_when_same_key_has_later_record_without_title() {
        let record_with_title = mk_domain_event_record();
        let key = record_with_title.key();
        let mut record_without_title = mk_domain_event_record();
        record_without_title.shop_id = key.shop_id;
        record_without_title.shops_product_id = key.shops_product_id.clone();
        record_without_title.title_native = None;

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.7, 0.8]) }));

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .withf(|cmds| cmds.len() == 1 && cmds.values().all(|cmd| cmd.embedding.is_some()))
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![
            mk_sqs_message(&record_with_title),
            mk_sqs_message(&record_without_title),
        ]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_all_messages_when_same_key_embedding_update_fails() {
        let record_with_title = mk_domain_event_record();
        let key = record_with_title.key();
        let mut record_without_title = mk_domain_event_record();
        record_without_title.shop_id = key.shop_id;
        record_without_title.shops_product_id = key.shops_product_id.clone();
        record_without_title.title_native = None;
        let message_id_1 = "embed-dup-1".to_string();
        let message_id_2 = "embed-dup-2".to_string();

        let mut mock_embedding_service = MockMultimodalEmbeddingService::default();
        mock_embedding_service
            .expect_embed()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.7, 0.8]) }));

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|cmds| Box::pin(async move { cmds }));

        let event = mk_lambda_event(vec![
            mk_sqs_message_with_id(&record_with_title, message_id_1.clone()),
            mk_sqs_message_with_id(&record_without_title, message_id_2.clone()),
        ]);
        let result = handler(&mock_embedding_service, &mock_command_service, event)
            .await
            .unwrap();

        let mut actual = result
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected = vec![message_id_1, message_id_2];
        expected.sort();
        assert_eq!(expected, actual);
    }

    #[allow(dead_code)]
    fn _assert_update_product_command_default() {
        // Compile-time check that UpdateProductCommand implements Default
        let _: UpdateProductCommand = UpdateProductCommand::default();
    }
}
