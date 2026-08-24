use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_listing_core::{product_id::ProductId, title::Title};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEmbeddingWrite {
    pub product_id: ProductId,
    pub source_event_id: EventId,
    pub enrichment_event_id: EventId,
    pub embedding: Vec<f32>,
    pub title: Localized<Language, Title>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingWriteOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductNotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductEmbeddingWriteError {
    #[error("product embedding write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductEmbeddingWriter: Send {
    async fn apply(
        &mut self,
        write: &ProductEmbeddingWrite,
    ) -> Result<ProductEmbeddingWriteOutcome, ProductEmbeddingWriteError>;
}

pub trait ProductEmbeddingWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductEmbeddingWriter + 'tx;
}
