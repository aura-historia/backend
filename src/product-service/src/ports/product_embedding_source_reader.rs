use common::{
    error::boxed::BoxError, event_id::EventId, language::domain::Language, localized::Localized,
    product_id::ProductId,
};
use product_core::{description::Description, title::Title};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEmbeddingSource {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub current_event_id: EventId,
    pub event_type: String,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub image_url: Option<Url>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductEmbeddingSourceReadError {
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
pub trait ProductEmbeddingSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_id: ProductId,
    ) -> Result<Option<ProductEmbeddingSource>, ProductEmbeddingSourceReadError>;
}
