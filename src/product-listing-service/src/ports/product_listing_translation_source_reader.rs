use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingTranslationSource {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub current_event_id: EventId,
    pub event_type: String,
    pub title: Option<Title>,
    pub title_language: Option<Language>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingTranslationSourceReadError {
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
pub trait ProductListingTranslationSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingTranslationSource>, ProductListingTranslationSourceReadError>;
}
