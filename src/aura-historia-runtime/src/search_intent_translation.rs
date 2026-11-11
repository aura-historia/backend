//! The saved-search intent translator shared by REST, MCP and periodic matching. It reuses the
//! provider and model that translate listing titles.

use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::SearchIntentTranslation;
use search_filter_service::ports::{SearchIntentTranslator, SearchIntentTranslatorError};
use search_filter_translation_llm::LargeLanguageModelSearchIntentTranslator;
use std::sync::Arc;

pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const EPHEMERAL_STAGE: &str = "ephemeral";

#[derive(Debug, thiserror::Error)]
pub enum SearchIntentTranslatorConfigError {
    #[error("missing required environment variable {ANTHROPIC_API_KEY_ENV}")]
    MissingApiKey,
    #[error("failed to configure the Anthropic search intent translator")]
    InvalidApiKey,
}

/// Builds the translator from `ANTHROPIC_API_KEY`. Only the ephemeral stage, which has no
/// provider credentials, may run without one; its saved searches then match the text as typed.
pub fn search_intent_translator<F>(
    mut get: F,
) -> Result<Arc<dyn SearchIntentTranslator>, SearchIntentTranslatorConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let api_key = get(ANTHROPIC_API_KEY_ENV).filter(|key| !key.trim().is_empty());
    let ephemeral = get("STAGE").as_deref() == Some(EPHEMERAL_STAGE);
    match api_key {
        Some(api_key) => Ok(Arc::new(
            LargeLanguageModelSearchIntentTranslator::anthropic(&api_key)
                .map_err(|_| SearchIntentTranslatorConfigError::InvalidApiKey)?,
        )),
        None if ephemeral => Ok(Arc::new(UnavailableSearchIntentTranslator)),
        None => Err(SearchIntentTranslatorConfigError::MissingApiKey),
    }
}

pub fn search_intent_translator_from_env()
-> Result<Arc<dyn SearchIntentTranslator>, SearchIntentTranslatorConfigError> {
    search_intent_translator(|name| std::env::var(name).ok())
}

/// A translator that is never available, for compositions without provider credentials.
/// Saved searches then match the user's original text, as after any translation failure.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableSearchIntentTranslator;

#[async_trait::async_trait]
impl SearchIntentTranslator for UnavailableSearchIntentTranslator {
    async fn translate(
        &self,
        _search: &ProductListingSearch,
    ) -> Result<SearchIntentTranslation, SearchIntentTranslatorError> {
        Err(SearchIntentTranslatorError::TemporarilyUnavailable {
            source: application::error::static_error("no search intent translator is configured"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator(
        values: &[(&'static str, &'static str)],
    ) -> Result<Arc<dyn SearchIntentTranslator>, SearchIntentTranslatorConfigError> {
        search_intent_translator(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn should_require_an_api_key_outside_the_ephemeral_stage() {
        for values in [
            &[][..],
            &[("STAGE", "prod")][..],
            &[("STAGE", "dev"), (ANTHROPIC_API_KEY_ENV, "  ")][..],
        ] {
            assert!(matches!(
                translator(values),
                Err(SearchIntentTranslatorConfigError::MissingApiKey)
            ));
        }
    }

    #[test]
    fn should_compose_with_an_api_key_or_in_the_ephemeral_stage() {
        assert!(translator(&[(ANTHROPIC_API_KEY_ENV, "fixture-key")]).is_ok());
        assert!(translator(&[("STAGE", "ephemeral")]).is_ok());
    }
}
