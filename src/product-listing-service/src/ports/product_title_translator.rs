use application::error::BoxError;
use indexmap::IndexMap;
use localization::Language;
use product_listing_core::title::Title;

#[derive(Debug, thiserror::Error)]
pub enum ProductTitleTranslationError {
    #[error("product title translation is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("product title translation response is invalid")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductTitleTranslator: Send + Sync {
    async fn translate(
        &self,
        title: &Title,
        source_language: Language,
        target_languages: &[Language],
    ) -> Result<IndexMap<Language, Title>, ProductTitleTranslationError>;
}
