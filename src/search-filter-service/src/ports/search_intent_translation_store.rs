use application::error::BoxError;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::SearchIntentTranslation;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use std::sync::Arc;

/// An active filter with semantic intent and no stored translation.
#[derive(Debug, Clone, PartialEq)]
pub struct UntranslatedSearchIntent {
    pub search_filter_id: UserSearchFilterId,
    pub version: i64,
    pub search: ProductListingSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UntranslatedSearchIntentPageRequest {
    pub after: Option<UserSearchFilterId>,
    pub page_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchIntentTranslationRecordOutcome {
    Recorded,
    /// The filter changed or was deleted since it was read; its next write or run translates it.
    FilterChanged,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchIntentTranslationStoreError {
    #[error("untranslated search intent read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("untranslated search intent persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("search intent translation write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

/// Translations for filters saved while the translator failed, and for filters saved before
/// translations existed. A recorded translation is internal matching state: it neither advances
/// the filter's version nor its `updated` time, so a frozen periodic run stays valid.
#[async_trait::async_trait]
pub trait SearchIntentTranslationStore: Send + Sync {
    async fn find_untranslated_page(
        &self,
        request: UntranslatedSearchIntentPageRequest,
    ) -> Result<Vec<UntranslatedSearchIntent>, SearchIntentTranslationStoreError>;

    /// Stores `translation` only while the filter is still at `expected_version`.
    async fn record(
        &self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        translation: &SearchIntentTranslation,
    ) -> Result<SearchIntentTranslationRecordOutcome, SearchIntentTranslationStoreError>;
}

#[async_trait::async_trait]
impl<S> SearchIntentTranslationStore for Arc<S>
where
    S: SearchIntentTranslationStore + ?Sized,
{
    async fn find_untranslated_page(
        &self,
        request: UntranslatedSearchIntentPageRequest,
    ) -> Result<Vec<UntranslatedSearchIntent>, SearchIntentTranslationStoreError> {
        self.as_ref().find_untranslated_page(request).await
    }

    async fn record(
        &self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        translation: &SearchIntentTranslation,
    ) -> Result<SearchIntentTranslationRecordOutcome, SearchIntentTranslationStoreError> {
        self.as_ref()
            .record(search_filter_id, expected_version, translation)
            .await
    }
}
