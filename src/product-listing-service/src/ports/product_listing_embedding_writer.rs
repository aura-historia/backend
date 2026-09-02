use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEmbeddingWrite {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    pub enrichment_event_id: EventId,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingEmbeddingWriteOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductListingNotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEmbeddingWriteError {
    #[error("product embedding write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingEmbeddingWriter: Send {
    async fn apply(
        &mut self,
        write: &ProductListingEmbeddingWrite,
    ) -> Result<ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriteError>;
}

pub trait ProductListingEmbeddingWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEmbeddingWriter + 'tx;
}
