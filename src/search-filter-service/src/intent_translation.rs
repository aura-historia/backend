//! Bounded English translation of a search's semantic intent for matching.
//!
//! A translation is an optimisation of matching quality, never a precondition: every failure
//! yields no translation, and matching then classifies the user's original text. Only metadata
//! is logged, never the original or translated text.

use crate::match_candidates::has_semantic_intent;
use crate::ports::SearchIntentTranslator;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::SearchIntentTranslation;
use std::time::{Duration, Instant};

/// Bound on one translation while a user waits for a filter write or a preview.
pub const SEARCH_INTENT_TRANSLATION_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranslationTrigger {
    Create,
    Update,
    Preview,
    Backfill,
}

impl TranslationTrigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Preview => "preview",
            Self::Backfill => "backfill",
        }
    }
}

/// The current translation of `search`'s semantic intent, or `None` when it has none or the
/// translator fails, times out or answers for other text.
pub(crate) async fn translate_search_intent<T>(
    translator: &T,
    search: &ProductListingSearch,
    trigger: TranslationTrigger,
) -> Option<SearchIntentTranslation>
where
    T: SearchIntentTranslator + ?Sized,
{
    if !has_semantic_intent(search) {
        return None;
    }
    let started = Instant::now();
    let result = tokio::time::timeout(
        SEARCH_INTENT_TRANSLATION_DEADLINE,
        translator.translate(search),
    )
    .await;
    let (translation, outcome, failure_category) = match result {
        Ok(Ok(translation)) if translation.is_current_for(search) => {
            let outcome = if translation.english_intent().is_some() {
                "translated"
            } else {
                "already_english"
            };
            (Some(translation), outcome, None)
        }
        Ok(Ok(_)) => (None, "failed", Some("fingerprint_mismatch")),
        Ok(Err(error)) => (None, "failed", Some(error.category())),
        Err(_) => (None, "failed", Some("timeout")),
    };
    tracing::info!(
        eventType = "SEARCH_INTENT_TRANSLATION",
        trigger = trigger.as_str(),
        outcome,
        failureCategory = failure_category,
        durationMs = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "Completed saved-search intent translation."
    );
    translation
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ports::SearchIntentTranslatorError;
    use application::error::static_error;
    use search_filter_core::EnglishSearchIntent;
    use std::sync::Mutex;

    /// Translates every query to `english <query>`; failure and staleness are opt-in.
    #[derive(Default)]
    pub(crate) struct StubTranslator {
        pub(crate) fail: bool,
        pub(crate) already_english: bool,
        pub(crate) translate_other_text: bool,
        pub(crate) calls: Mutex<usize>,
    }

    impl StubTranslator {
        pub(crate) fn failing() -> Self {
            Self {
                fail: true,
                ..Self::default()
            }
        }

        pub(crate) fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    pub(crate) fn english_of(search: &ProductListingSearch) -> EnglishSearchIntent {
        EnglishSearchIntent::new(
            search
                .product_listing_query
                .iter()
                .map(|query| format!("english {}", query.as_ref()).try_into().unwrap())
                .collect(),
            search
                .enhanced_search_description
                .as_ref()
                .map(|description| {
                    format!("english {}", description.as_ref())
                        .as_str()
                        .try_into()
                        .unwrap()
                }),
        )
    }

    #[async_trait::async_trait]
    impl SearchIntentTranslator for StubTranslator {
        async fn translate(
            &self,
            search: &ProductListingSearch,
        ) -> Result<SearchIntentTranslation, SearchIntentTranslatorError> {
            *self.calls.lock().unwrap() += 1;
            if self.fail {
                return Err(SearchIntentTranslatorError::TemporarilyUnavailable {
                    source: static_error("translator offline"),
                });
            }
            if self.already_english {
                return Ok(SearchIntentTranslation::already_english(search));
            }
            let mut source = search.clone();
            if self.translate_other_text {
                source.enhanced_search_description = Some("other text".try_into().unwrap());
                source.product_listing_query.clear();
            }
            Ok(SearchIntentTranslation::english(&source, english_of(&source)).unwrap())
        }
    }

    fn search(query: &str) -> ProductListingSearch {
        ProductListingSearch::new(localization::Language::En, money::Currency::Eur)
            .with_product_listing_query(query.try_into().unwrap())
    }

    #[tokio::test]
    async fn should_translate_only_searches_with_semantic_intent() {
        let translator = StubTranslator::default();
        let without_text =
            ProductListingSearch::new(localization::Language::De, money::Currency::Eur);

        assert_eq!(
            None,
            translate_search_intent(&translator, &without_text, TranslationTrigger::Create).await
        );
        assert_eq!(0, translator.calls());
    }

    #[tokio::test]
    async fn should_yield_no_translation_when_the_translator_fails() {
        let search = search("Manschettenknöpfe");

        assert_eq!(
            None,
            translate_search_intent(
                &StubTranslator::failing(),
                &search,
                TranslationTrigger::Update
            )
            .await
        );
    }

    #[tokio::test]
    async fn should_discard_a_translation_of_other_text() {
        let translator = StubTranslator {
            translate_other_text: true,
            ..StubTranslator::default()
        };

        assert_eq!(
            None,
            translate_search_intent(
                &translator,
                &search("Broschen"),
                TranslationTrigger::Backfill
            )
            .await
        );
    }

    struct HangingTranslator;

    #[async_trait::async_trait]
    impl SearchIntentTranslator for HangingTranslator {
        async fn translate(
            &self,
            _search: &ProductListingSearch,
        ) -> Result<SearchIntentTranslation, SearchIntentTranslatorError> {
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_give_up_after_the_interactive_deadline() {
        assert_eq!(
            None,
            translate_search_intent(
                &HangingTranslator,
                &search("Vase"),
                TranslationTrigger::Preview
            )
            .await
        );
    }
}
