use application::error::{box_error, static_error};
use indexmap::IndexMap;
use large_language_model::LlmOperation;
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use localization::Language;
use product_core::title::Title;
use product_service::ports::{ProductTitleTranslationError, ProductTitleTranslator};
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};

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
                    source: static_error(
                        "product title translation response omitted a target language",
                    ),
                });
            };
            let title = Title::from(value.as_str());
            if title.as_ref().is_empty() {
                return Err(ProductTitleTranslationError::InvalidResponse {
                    source: static_error(
                        "product title translation response contains an empty title",
                    ),
                });
            }
            translated.insert(*language, title);
        }
        Ok(translated)
    }
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
        response_json_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "titles": {
                    "type": "object",
                    "properties": targets.iter().map(|language| (
                        (*language).to_owned(),
                        serde_json::json!({ "type": "string" }),
                    )).collect::<serde_json::Map<String, serde_json::Value>>(),
                    "required": targets,
                }
            },
            "required": ["titles"],
        }),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 512,
            request_timeout: Duration::from_secs(30),
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
        LargeLanguageModelError::InvalidRequest { .. }
        | LargeLanguageModelError::Permanent { .. }
        | LargeLanguageModelError::InvalidResponse { .. } => {
            ProductTitleTranslationError::InvalidResponse {
                source: box_error(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_schema_for_each_requested_target_language() {
        let request = translation_request(
            &Title::from("Antiker Stuhl"),
            Language::De,
            &[Language::En, Language::Fr],
        );

        assert_eq!(
            Some("object"),
            request
                .response_json_schema
                .get("type")
                .and_then(serde_json::Value::as_str)
        );
        assert!(request.prompt.contains("Antiker Stuhl"));
        assert!(!request.prompt.contains("de, de"));
    }
}
