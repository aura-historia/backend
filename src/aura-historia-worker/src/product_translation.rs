use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    language::domain::Language,
    logging::LlmOperation,
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    product_id::ProductId,
};
use indexmap::IndexMap;
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use product_core::title::Title;
use product_service::{
    ports::{ProductTitleTranslationError, ProductTitleTranslator},
    use_cases::{
        TranslateProductEventCommand, TranslateProductEventOutcome, TranslateProductEventUseCase,
    },
};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;
use tracing::{error, info};

const TRANSLATION_SYSTEM_INSTRUCTION: &str = "Translate antique product titles faithfully. Preserve proper nouns, periods, dimensions, and material names.";

pub struct LargeLanguageModelProductTitleTranslator<L> {
    large_language_model: L,
}

impl<L> LargeLanguageModelProductTitleTranslator<L> {
    pub fn new(large_language_model: L) -> Self {
        Self {
            large_language_model,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TranslationResponse {
    titles: BTreeMap<String, String>,
}

#[async_trait::async_trait]
impl<L> ProductTitleTranslator for LargeLanguageModelProductTitleTranslator<L>
where
    L: LargeLanguageModel,
{
    async fn translate(
        &self,
        title: &Title,
        source_language: Language,
        target_languages: &[Language],
    ) -> Result<IndexMap<Language, Title>, ProductTitleTranslationError> {
        let response: TranslationResponse = self
            .large_language_model
            .generate(translation_request(
                title,
                source_language,
                target_languages,
            ))
            .await
            .map_err(map_translation_error)?;
        let mut translated = IndexMap::new();
        for language in target_languages {
            let Some(value) = response.titles.get(language.as_str()) else {
                return Err(ProductTitleTranslationError::InvalidResponse {
                    source: common::error::boxed::static_error(
                        "product title translation response omitted a target language",
                    ),
                });
            };
            let title = Title::from(value.as_str());
            if title.as_ref().is_empty() {
                return Err(ProductTitleTranslationError::InvalidResponse {
                    source: common::error::boxed::static_error(
                        "product title translation response contains an empty title",
                    ),
                });
            }
            translated.insert(*language, title);
        }
        Ok(translated)
    }
}

pub async fn consume_product_translation_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn TranslateProductEventUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();
    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let use_case_for_retry = Arc::clone(&use_case);
        let outcome = Arc::new(Mutex::new(None));
        let outcome_for_retry = Arc::clone(&outcome);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let use_case = Arc::clone(&use_case_for_retry);
            let outcome = Arc::clone(&outcome_for_retry);
            async move { execute_job(use_case, job, outcome).await }
        })
        .await;
        match (result, outcome.lock().await.take()) {
            (Ok(()), Some(outcome)) => info!(
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                ?outcome,
                "product translation job completed"
            ),
            (Ok(()), None) => error!(
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                outcome = "missing",
                "product translation job completed without an outcome"
            ),
            (Err(error), _) => error!(
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "product translation job failed"
            ),
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn TranslateProductEventUseCase>,
    job: DomainJob,
    outcome: Arc<Mutex<Option<TranslateProductEventOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-translation:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    let result = use_case
        .execute(&context, command)
        .await
        .map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
}

fn command_from_job(
    job: DomainJob,
) -> Result<TranslateProductEventCommand, ProductTranslationWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(ProductTranslationWorkerError::UnexpectedJobPayload);
    };
    let event_id = EventId::try_from(event.event_id.as_str()).map_err(|source| {
        ProductTranslationWorkerError::InvalidEventId {
            source: box_error(source),
        }
    })?;
    let product_id = ProductId::try_from(event.product_id.as_str()).map_err(|source| {
        ProductTranslationWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;
    Ok(TranslateProductEventCommand {
        event_id,
        product_id,
    })
}

fn translation_request(
    title: &Title,
    source_language: Language,
    target_languages: &[Language],
) -> StructuredGenerationRequest {
    let targets = target_languages
        .iter()
        .map(|language| language.as_str())
        .collect::<Vec<_>>();
    StructuredGenerationRequest {
        operation: LlmOperation::ProductTitleTranslation,
        system_instruction: TRANSLATION_SYSTEM_INSTRUCTION.to_owned(),
        prompt: format!(
            "Translate this antique product title from {source} into {targets}. Return exactly one JSON object with a titles object whose keys are the target ISO language codes and whose values are non-empty translated titles.\n\nTitle: {title}",
            source = source_language.as_str(),
            targets = targets.join(", "),
            title = title.as_ref(),
        ),
        image_urls: Vec::new(),
        response_schema: serde_json::json!({
            "type": "OBJECT",
            "properties": {
                "titles": {
                    "type": "OBJECT",
                    "properties": targets.iter().map(|language| (
                        (*language).to_owned(),
                        serde_json::json!({ "type": "STRING" }),
                    )).collect::<serde_json::Map<String, serde_json::Value>>(),
                    "required": targets,
                }
            },
            "required": ["titles"],
        }),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 512,
        },
    }
}

fn map_translation_error(error: LargeLanguageModelError) -> ProductTitleTranslationError {
    match error {
        LargeLanguageModelError::Timeout { .. }
        | LargeLanguageModelError::Retryable { .. }
        | LargeLanguageModelError::Authentication { .. } => {
            ProductTitleTranslationError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
        LargeLanguageModelError::Permanent { .. }
        | LargeLanguageModelError::InvalidResponse { .. } => {
            ProductTitleTranslationError::InvalidResponse {
                source: box_error(error),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ProductTranslationWorkerError {
    #[error("product translation queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("product translation job has an invalid event id")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("product translation job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductEventJob, WorkerQueue};

    fn job(event_id: &str, product_id: &str) -> DomainJob {
        DomainJob {
            target_queue: WorkerQueue::ProductTranslate,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductEvent(ProductEventJob {
                event_id: event_id.to_owned(),
                product_id: product_id.to_owned(),
                event_type: "ENRICHMENT_EMBEDDED".to_owned(),
                event_group: "ENRICHMENT".to_owned(),
            }),
        }
    }

    #[test]
    fn should_map_product_event_job_to_translation_command() {
        let product_id = ProductId::new();
        let event_id = EventId::new();

        let command = command_from_job(job(&event_id.to_string(), &product_id.to_string()));

        assert!(matches!(
            command,
            Ok(TranslateProductEventCommand { event_id: actual_event_id, product_id: actual_product_id })
                if actual_event_id == event_id && actual_product_id == product_id
        ));
    }

    #[test]
    fn should_create_schema_for_each_requested_target_language() {
        let request = translation_request(
            &Title::from("Antiker Stuhl"),
            Language::De,
            &[Language::En, Language::Fr],
        );

        assert_eq!(
            Some("OBJECT"),
            request
                .response_schema
                .get("type")
                .and_then(serde_json::Value::as_str)
        );
        assert!(request.prompt.contains("Antiker Stuhl"));
        assert!(!request.prompt.contains("de, de"));
    }
}
