use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_listing_core::{
    description::Description, product_listing_id::ProductListingId, title::Title,
};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEmbeddingSource {
    pub product_id: ProductListingId,
    pub event_id: EventId,
    pub current_event_id: EventId,
    pub event_type: String,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub image_url: Option<Url>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEmbeddingSourceReadError {
    #[error("product embedding source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product embedding source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingEmbeddingSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductListingId,
    ) -> Result<Option<ProductListingEmbeddingSource>, ProductListingEmbeddingSourceReadError>;
}
