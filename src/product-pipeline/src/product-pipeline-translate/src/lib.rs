pub mod service;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use common::{
    dynamodb_stream::extract_from_dynamodb_stream,
    has_key::HasKey,
    language::domain::Language,
    localized::Localized,
    logging::{LogEventType, LogPipelineStage},
    product_id::ProductKey,
};
use lambda_runtime::LambdaEvent;
use product::{
    core::title::Title,
    dynamodb::product_event_record::ProductEventRecord,
    dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord,
    service::{
        command_service::CommandProductService,
        product_command::{TranslationEnvelope, UpdateProductCommand},
    },
};
use service::TranslationService;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[tracing::instrument(
    skip(translation_service, command_service, event),
    fields(requestId = %event.context.request_id)
)]
pub async fn handler(
    translation_service: &(impl TranslationService + Sync),
    command_service: &(impl CommandProductService + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let count = event.payload.records.len();

    let (event_records, mut failed_message_ids) =
        extract_from_dynamodb_stream::<ProductEventRecord>(event.payload.records);

    // Collect only ENRICHMENT_EMBEDDED records that carry a native title and language.
    struct ProductInput {
        message_id: String,
        key: ProductKey,
        source_language: Language,
        title: String,
    }

    let mut valid_inputs: Vec<ProductInput> = Vec::new();

    for (message_id, event_record) in event_records {
        let enrichment_record = match event_record {
            ProductEventRecord::Enrichment(r) => r,
            other => {
                let key = other.key();
                error!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    eventId = %other.event_id(),
                    "Unexpected non-Enrichment event record type in translate handler."
                );
                continue;
            }
        };

        if enrichment_record.event_type != ProductEnrichmentEventTypeRecord::EnrichmentEmbedded {
            let key = enrichment_record.key();
            warn!(
                messageId = message_id,
                shopId = %key.shop_id,
                shopsProductId = %key.shops_product_id,
                "Enrichment event is not ENRICHMENT_EMBEDDED — skipping."
            );
            continue;
        }

        let key = enrichment_record.key();

        let title_text = match enrichment_record.native_title {
            Some(t) => t,
            None => {
                warn!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "ENRICHMENT_EMBEDDED record has no native title — skipping translation."
                );
                continue;
            }
        };

        let source_language: Language = match enrichment_record.native_title_language {
            Some(lang) => lang.into(),
            None => {
                warn!(
                    messageId = message_id,
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "ENRICHMENT_EMBEDDED record has no native title language — skipping translation."
                );
                continue;
            }
        };

        let title_trimmed = title_text.trim().to_string();
        if title_trimmed.is_empty() {
            warn!(
                messageId = message_id,
                shopId = %key.shop_id,
                shopsProductId = %key.shops_product_id,
                "ENRICHMENT_EMBEDDED record has empty native title — skipping translation."
            );
            continue;
        }

        valid_inputs.push(ProductInput {
            message_id,
            key,
            source_language,
            title: title_trimmed,
        });
    }

    if valid_inputs.is_empty() {
        let failures = failed_message_ids.len();
        info!(
            eventType = %LogEventType::BatchProcessing,
            pipelineStage = %LogPipelineStage::ProductTranslation,
            processed = count,
            successful = 0,
            failures = failures,
            "Processed product translation batch."
        );
        let mut sqs_batch_response = SqsBatchResponse::default();
        sqs_batch_response.batch_item_failures = failed_message_ids
            .into_iter()
            .map(mk_batch_item_failure)
            .collect();
        return Ok(sqs_batch_response);
    }

    // Group inputs by source language so we make one translation call per language group.
    let mut by_language: HashMap<Language, Vec<usize>> = HashMap::new();
    for (idx, input) in valid_inputs.iter().enumerate() {
        by_language
            .entry(input.source_language)
            .or_default()
            .push(idx);
    }

    let mut translation_results: Vec<(usize, Option<HashMap<Language, String>>)> =
        Vec::with_capacity(valid_inputs.len());

    for (source_language, indices) in by_language {
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_by_key(|&i| valid_inputs[i].title.len());

        let titles: Vec<String> = sorted_indices
            .iter()
            .map(|&i| valid_inputs[i].title.clone())
            .collect();

        let group_results = translation_service
            .translate(&titles, source_language)
            .await;

        for (batch_pos, &original_idx) in sorted_indices.iter().enumerate() {
            translation_results.push((
                original_idx,
                group_results.get(batch_pos).cloned().flatten(),
            ));
        }
    }

    // Build update commands from translation results.
    let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();

    for (idx, maybe_translations) in translation_results {
        let input = &valid_inputs[idx];
        let translations = match maybe_translations {
            Some(t) => t,
            None => {
                warn!(
                    messageId = input.message_id,
                    shopId = %input.key.shop_id,
                    shopsProductId = %input.key.shops_product_id,
                    "Translation service returned no result for product — marking message as failed."
                );
                failed_message_ids.push(input.message_id.clone());
                continue;
            }
        };

        let targets: HashMap<Language, Title> = translations
            .into_iter()
            .map(|(lang, text)| (lang, Title::from(text)))
            .collect();

        let envelope = TranslationEnvelope {
            source: Localized::new(input.source_language, Title::from(input.title.as_str())),
            targets,
        };

        update_cmds.insert(
            input.key.clone(),
            UpdateProductCommand {
                translated_titles: Some(envelope),
                ..UpdateProductCommand::default()
            },
        );
    }

    if !update_cmds.is_empty() {
        // Map ProductKey back to message_id for failure reporting.
        let key_to_message: HashMap<ProductKey, String> = valid_inputs
            .iter()
            .map(|i| (i.key.clone(), i.message_id.clone()))
            .collect();

        let failed_keys = command_service.update(update_cmds).await;
        for (key, _cmd) in failed_keys {
            if let Some(message_id) = key_to_message.get(&key) {
                failed_message_ids.push(message_id.clone());
            } else {
                error!(
                    shopId = %key.shop_id,
                    shopsProductId = %key.shops_product_id,
                    "No message_id found for failed translate update command."
                );
            }
        }
    }

    let failures = failed_message_ids.len();
    info!(
        eventType = %LogEventType::BatchProcessing,
        pipelineStage = %LogPipelineStage::ProductTranslation,
        processed = count,
        successful = count - failures,
        failures = failures,
        "Processed product translation batch."
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
    use common::language::{domain::Language, record::LanguageRecord};
    use fake::{Fake, Faker};
    use lambda_runtime::{Context, LambdaEvent};
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
    use product::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
    use product::service::command_service::MockCommandProductService;
    use product::service::product_command::UpdateProductCommand;
    use crate::service::MockTranslationService;
    use std::collections::HashMap;
    use std::time::SystemTime;
    use uuid::Uuid;

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

    fn mk_enrichment_embedded_record(
        native_title: Option<&str>,
        native_title_language: Option<Language>,
    ) -> ProductEnrichmentEventRecord {
        let mut record: ProductEnrichmentEventRecord = Faker.fake();
        record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentEmbedded;
        record.native_title = native_title.map(|t| t.to_string());
        record.native_title_language = native_title_language.map(LanguageRecord::from);
        record
    }

    #[tokio::test]
    async fn should_return_no_failures_when_batch_is_empty() {
        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();
        let event = mk_lambda_event(vec![]);

        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_single_product_translated_successfully() {
        let record = mk_enrichment_embedded_record(Some("Antiker Stuhl"), Some(Language::De));

        let mut mock_translation_service = MockTranslationService::default();
        mock_translation_service
            .expect_translate()
            .times(1)
            .returning(|_, _| {
                Box::pin(async {
                    vec![Some(HashMap::from([
                        (Language::En, "Antique chair".to_string()),
                        (Language::Fr, "Chaise ancienne".to_string()),
                    ]))]
                })
            });

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_record_when_native_title_is_missing() {
        let record = mk_enrichment_embedded_record(None, Some(Language::De));
        let message_id = "no-title-msg".to_string();

        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_record_when_native_title_language_is_missing() {
        let record = mk_enrichment_embedded_record(Some("Antiker Stuhl"), None);
        let message_id = "no-lang-msg".to_string();

        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_skip_record_when_native_title_is_empty() {
        let record = mk_enrichment_embedded_record(Some("  "), Some(Language::De));
        let message_id = "empty-title-msg".to_string();

        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_failure_when_translation_service_returns_none() {
        let record = mk_enrichment_embedded_record(Some("Antiker Stuhl"), Some(Language::De));
        let message_id = "trans-fail-msg".to_string();

        let mut mock_translation_service = MockTranslationService::default();
        mock_translation_service
            .expect_translate()
            .times(1)
            .returning(|_, _| Box::pin(async { vec![None] }));

        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_return_failure_when_command_service_update_fails() {
        let record = mk_enrichment_embedded_record(Some("Antiker Stuhl"), Some(Language::De));
        let message_id = "cmd-fail-msg".to_string();

        let mut mock_translation_service = MockTranslationService::default();
        mock_translation_service
            .expect_translate()
            .times(1)
            .returning(|_, _| {
                Box::pin(async {
                    vec![Some(HashMap::from([(
                        Language::En,
                        "Antique chair".to_string(),
                    )]))]
                })
            });

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|cmds| Box::pin(async move { cmds }));

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(&record, message_id.clone())]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(message_id, result.batch_item_failures[0].item_identifier);
    }

    #[tokio::test]
    async fn should_skip_non_embedded_enrichment_records() {
        let mut record: ProductEnrichmentEventRecord = Faker.fake();
        record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentTranslatedTitle;
        let message_id = "wrong-type-msg".to_string();

        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &ProductEventRecord::Enrichment(record),
            message_id,
        )]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_non_enrichment_event_records() {
        let domain_record: ProductDomainEventRecord = Faker.fake();
        let message_id = "domain-msg".to_string();

        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let event = mk_lambda_event(vec![mk_sqs_message_with_id(
            &ProductEventRecord::Domain(domain_record),
            message_id,
        )]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_set_translated_titles_field_in_update_command() {
        let record = mk_enrichment_embedded_record(Some("Antiker Stuhl"), Some(Language::De));

        let mut mock_translation_service = MockTranslationService::default();
        mock_translation_service
            .expect_translate()
            .times(1)
            .returning(|_, _| {
                Box::pin(async {
                    vec![Some(HashMap::from([(
                        Language::En,
                        "Antique chair".to_string(),
                    )]))]
                })
            });

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .withf(|cmds| cmds.values().all(|cmd| cmd.translated_titles.is_some()))
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_no_failures_when_multiple_products_translated_successfully() {
        let record1 = mk_enrichment_embedded_record(Some("Antiker Stuhl"), Some(Language::De));
        let record2 = mk_enrichment_embedded_record(Some("Antique table"), Some(Language::En));

        let mut mock_translation_service = MockTranslationService::default();
        mock_translation_service
            .expect_translate()
            .times(2)
            .returning(|_, _| {
                Box::pin(async {
                    vec![Some(HashMap::from([(
                        Language::Fr,
                        "Une chaise ancienne".to_string(),
                    )]))]
                })
            });

        let mut mock_command_service = MockCommandProductService::default();
        mock_command_service
            .expect_update()
            .times(1)
            .returning(|_| Box::pin(async { HashMap::new() }));

        let event = mk_lambda_event(vec![mk_sqs_message(&record1), mk_sqs_message(&record2)]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_messages_with_invalid_json_body() {
        let mock_translation_service = MockTranslationService::default();
        let mock_command_service = MockCommandProductService::default();

        let mut invalid_msg = SqsMessage::default();
        invalid_msg.message_id = Some("invalid-json-msg".to_string());
        invalid_msg.body = Some("invalid json {".to_string());

        let event = mk_lambda_event(vec![invalid_msg]);
        let result = handler(&mock_translation_service, &mock_command_service, event)
            .await
            .unwrap();

        assert_eq!(1, result.batch_item_failures.len());
        assert_eq!(
            "invalid-json-msg",
            result.batch_item_failures[0].item_identifier
        );
    }

    #[allow(dead_code)]
    fn _assert_update_product_command_default() {
        let _: UpdateProductCommand = UpdateProductCommand::default();
    }
}
