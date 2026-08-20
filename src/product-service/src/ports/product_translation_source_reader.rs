use common::{error::boxed::BoxError, event_id::EventId};
use localization::Language;
use product_core::{product_id::ProductId, title::Title};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductTranslationSource {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub current_event_id: EventId,
    pub event_type: String,
    pub title: Option<Title>,
    pub title_language: Option<Language>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductTranslationSourceReadError {
    #[error("product translation source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product translation source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductTranslationSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductTranslationSource>, ProductTranslationSourceReadError>;
}
