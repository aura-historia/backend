use application::error::BoxError;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::SearchIntentTranslation;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SearchIntentTranslatorError {
    #[error("search intent translator is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("search intent translator returned an unusable translation")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

impl SearchIntentTranslatorError {
    pub const fn category(&self) -> &'static str {
        match self {
            Self::TemporarilyUnavailable { .. } => "temporarily_unavailable",
            Self::InvalidResponse { .. } => "invalid_response",
        }
    }
}

/// Translates a search's semantic intent, its base product queries and enhanced description,
/// into English for matching. The source language is detected from the text, never taken from
/// the search's UI language; English text yields [`SearchIntentTranslation::already_english`].
/// Callers only pass searches with semantic intent.
#[async_trait::async_trait]
pub trait SearchIntentTranslator: Send + Sync {
    async fn translate(
        &self,
        search: &ProductListingSearch,
    ) -> Result<SearchIntentTranslation, SearchIntentTranslatorError>;
}

#[async_trait::async_trait]
impl<T> SearchIntentTranslator for Arc<T>
where
    T: SearchIntentTranslator + ?Sized,
{
    async fn translate(
        &self,
        search: &ProductListingSearch,
    ) -> Result<SearchIntentTranslation, SearchIntentTranslatorError> {
        self.as_ref().translate(search).await
    }
}
