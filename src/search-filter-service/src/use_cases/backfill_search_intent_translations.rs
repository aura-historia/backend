//! Translates the semantic intent of active filters that have no translation yet: filters saved
//! while the translator failed, and filters saved before translations existed. Periodic matching
//! runs it first, so the run it is about to freeze already classifies the English intent.

use crate::intent_translation::{TranslationTrigger, translate_search_intent};
use crate::match_candidates::has_semantic_intent;
use crate::ports::{
    SearchIntentTranslationRecordOutcome, SearchIntentTranslationStore,
    SearchIntentTranslationStoreError, SearchIntentTranslator, UntranslatedSearchIntentPageRequest,
};
use futures::StreamExt;
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchIntentTranslationBackfillPolicy {
    pub page_size: NonZeroUsize,
    /// Translation attempts per run, so a translator outage cannot hold up matching for long.
    /// Filters beyond it are translated by a later run.
    pub max_translations: usize,
    pub concurrency: NonZeroUsize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchIntentTranslationBackfillReport {
    pub filters_read: usize,
    pub translations_recorded: usize,
    pub translation_failures: usize,
    pub filters_changed: usize,
    pub attempt_limit_reached: bool,
}

#[async_trait::async_trait]
pub trait BackfillSearchIntentTranslationsUseCase: Send + Sync {
    async fn execute(
        &self,
    ) -> Result<SearchIntentTranslationBackfillReport, SearchIntentTranslationStoreError>;
}

pub struct BackfillSearchIntentTranslationsHandler<S, T> {
    store: S,
    translator: T,
    policy: SearchIntentTranslationBackfillPolicy,
}

impl<S, T> BackfillSearchIntentTranslationsHandler<S, T> {
    pub fn new(store: S, translator: T, policy: SearchIntentTranslationBackfillPolicy) -> Self {
        Self {
            store,
            translator,
            policy,
        }
    }
}

#[async_trait::async_trait]
impl<S, T> BackfillSearchIntentTranslationsUseCase for BackfillSearchIntentTranslationsHandler<S, T>
where
    S: SearchIntentTranslationStore,
    T: SearchIntentTranslator,
{
    #[tracing::instrument(name = "backfill_search_intent_translations", skip_all)]
    async fn execute(
        &self,
    ) -> Result<SearchIntentTranslationBackfillReport, SearchIntentTranslationStoreError> {
        let mut report = SearchIntentTranslationBackfillReport::default();
        let mut after = None;
        loop {
            let remaining = self
                .policy
                .max_translations
                .saturating_sub(report.filters_read);
            if remaining == 0 {
                report.attempt_limit_reached = true;
                break;
            }
            let page = self
                .store
                .find_untranslated_page(UntranslatedSearchIntentPageRequest {
                    after,
                    page_size: self.policy.page_size.get().min(remaining),
                })
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            after = Some(last.search_filter_id);
            report.filters_read += page.len();
            let translator = &self.translator;
            let translations = page
                .into_iter()
                .filter(|filter| has_semantic_intent(&filter.search))
                .map(|filter| async move {
                    let translation = translate_search_intent(
                        translator,
                        &filter.search,
                        TranslationTrigger::Backfill,
                    )
                    .await;
                    (filter, translation)
                })
                .collect::<Vec<_>>();
            let translated = futures::stream::iter(translations)
                .buffer_unordered(self.policy.concurrency.get())
                .collect::<Vec<_>>()
                .await;
            for (filter, translation) in translated {
                let Some(translation) = translation else {
                    report.translation_failures += 1;
                    continue;
                };
                match self
                    .store
                    .record(filter.search_filter_id, filter.version, &translation)
                    .await?
                {
                    SearchIntentTranslationRecordOutcome::Recorded => {
                        report.translations_recorded += 1;
                    }
                    SearchIntentTranslationRecordOutcome::FilterChanged => {
                        report.filters_changed += 1;
                    }
                }
            }
        }
        tracing::info!(
            filters_read = report.filters_read,
            translations_recorded = report.translations_recorded,
            translation_failures = report.translation_failures,
            filters_changed = report.filters_changed,
            attempt_limit_reached = report.attempt_limit_reached,
            "search_filter.intent_translation_backfill.completed"
        );
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent_translation::tests::StubTranslator;
    use crate::ports::UntranslatedSearchIntent;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_search::ProductListingSearch;
    use search_filter_core::SearchIntentTranslation;
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MemoryStore {
        filters: Mutex<Vec<UntranslatedSearchIntent>>,
        recorded: Mutex<Vec<(UserSearchFilterId, i64, SearchIntentTranslation)>>,
        changed: Vec<UserSearchFilterId>,
    }

    impl MemoryStore {
        fn with_filters(queries: &[&str]) -> Self {
            let mut filters = queries
                .iter()
                .map(|query| UntranslatedSearchIntent {
                    search_filter_id: UserSearchFilterId::new(),
                    version: 3,
                    search: ProductListingSearch::new(Language::En, Currency::Eur)
                        .with_product_listing_query((*query).try_into().unwrap()),
                })
                .collect::<Vec<_>>();
            filters.sort_by_key(|filter| filter.search_filter_id.into_uuid());
            Self {
                filters: Mutex::new(filters),
                ..Self::default()
            }
        }
    }

    #[async_trait::async_trait]
    impl SearchIntentTranslationStore for MemoryStore {
        async fn find_untranslated_page(
            &self,
            request: UntranslatedSearchIntentPageRequest,
        ) -> Result<Vec<UntranslatedSearchIntent>, SearchIntentTranslationStoreError> {
            let recorded = self.recorded.lock().unwrap();
            Ok(self
                .filters
                .lock()
                .unwrap()
                .iter()
                .filter(|filter| {
                    request
                        .after
                        .is_none_or(|after| filter.search_filter_id.into_uuid() > after.into_uuid())
                })
                .filter(|filter| {
                    !recorded
                        .iter()
                        .any(|(id, _, _)| *id == filter.search_filter_id)
                })
                .take(request.page_size)
                .cloned()
                .collect())
        }

        async fn record(
            &self,
            search_filter_id: UserSearchFilterId,
            expected_version: i64,
            translation: &SearchIntentTranslation,
        ) -> Result<SearchIntentTranslationRecordOutcome, SearchIntentTranslationStoreError>
        {
            if self.changed.contains(&search_filter_id) {
                return Ok(SearchIntentTranslationRecordOutcome::FilterChanged);
            }
            self.recorded.lock().unwrap().push((
                search_filter_id,
                expected_version,
                translation.clone(),
            ));
            Ok(SearchIntentTranslationRecordOutcome::Recorded)
        }
    }

    fn policy(max_translations: usize) -> SearchIntentTranslationBackfillPolicy {
        SearchIntentTranslationBackfillPolicy {
            page_size: NonZeroUsize::new(2).unwrap(),
            max_translations,
            concurrency: NonZeroUsize::new(2).unwrap(),
        }
    }

    #[tokio::test]
    async fn should_record_current_translations_with_the_read_version() {
        let store = Arc::new(MemoryStore::with_filters(&["Vase", "Kommode", "Leuchter"]));
        let handler = BackfillSearchIntentTranslationsHandler::new(
            Arc::clone(&store),
            StubTranslator::default(),
            policy(10),
        );

        let report = handler.execute().await.unwrap();

        assert_eq!(3, report.translations_recorded);
        assert!(!report.attempt_limit_reached);
        let filters = store.filters.lock().unwrap();
        for (id, version, translation) in store.recorded.lock().unwrap().iter() {
            let filter = filters
                .iter()
                .find(|filter| filter.search_filter_id == *id)
                .unwrap();
            assert_eq!(3, *version);
            assert!(translation.is_current_for(&filter.search));
        }
    }

    #[tokio::test]
    async fn should_record_nothing_and_keep_going_when_the_translator_fails() {
        let store = Arc::new(MemoryStore::with_filters(&["Vase", "Kommode", "Leuchter"]));
        let translator = Arc::new(StubTranslator::failing());
        let handler = BackfillSearchIntentTranslationsHandler::new(
            Arc::clone(&store),
            Arc::clone(&translator),
            policy(10),
        );

        let report = handler.execute().await.unwrap();

        assert_eq!(3, report.translation_failures);
        assert_eq!(3, translator.calls());
        assert!(store.recorded.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_stop_at_the_attempt_limit() {
        let store = Arc::new(MemoryStore::with_filters(&["Vase", "Kommode", "Leuchter"]));
        let translator = Arc::new(StubTranslator::failing());
        let handler = BackfillSearchIntentTranslationsHandler::new(
            Arc::clone(&store),
            Arc::clone(&translator),
            policy(2),
        );

        let report = handler.execute().await.unwrap();

        assert_eq!(2, translator.calls());
        assert!(report.attempt_limit_reached);
    }

    #[tokio::test]
    async fn should_count_filters_changed_since_they_were_read() {
        let mut store = MemoryStore::with_filters(&["Vase"]);
        store.changed = vec![store.filters.lock().unwrap()[0].search_filter_id];
        let handler = BackfillSearchIntentTranslationsHandler::new(
            Arc::new(store),
            StubTranslator::default(),
            policy(10),
        );

        let report = handler.execute().await.unwrap();

        assert_eq!(1, report.filters_changed);
        assert_eq!(0, report.translations_recorded);
    }
}
