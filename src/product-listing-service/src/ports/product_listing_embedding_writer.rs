use crate::ports::StagedProductImage;
use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use url::Url;

/// A vector computed for the embedding revision `source_event_id`. Applying it also records the
/// revision's title, description and first image as the input behind the vector.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEmbeddingWrite {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    pub enrichment_event_id: EventId,
    pub embedding: Vec<f32>,
    /// The revision's first image, when the vector was computed with it.
    pub image: Option<StagedProductImage>,
}

/// Records that the listing's current vector also stands for the later embedding revision
/// `source_event_id`, whose input is unchanged from the one recorded for
/// `embedded_source_event_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEmbeddingConfirmation {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    /// The revision the recorded input stood for when the revision was compared with it.
    pub embedded_source_event_id: EventId,
    /// The revision's first image URL, which serves the embedded picture.
    pub image_url: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingEmbeddingConfirmOutcome {
    /// The vector stands for the revision, now or already.
    Confirmed,
    /// The listing moved past the revision, or is gone.
    Superseded,
    /// The recorded input is no longer the one the revision was compared with.
    EmbeddedInputChanged,
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

    /// Keeps the vector and its recorded input; neither appends an event nor touches the
    /// listing's projection, because what it shows does not change.
    async fn confirm(
        &mut self,
        confirmation: &ProductListingEmbeddingConfirmation,
    ) -> Result<ProductListingEmbeddingConfirmOutcome, ProductListingEmbeddingWriteError>;
}

pub trait ProductListingEmbeddingWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEmbeddingWriter + 'tx;
}
