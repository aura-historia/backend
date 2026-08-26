use application::error::BoxError;
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use product_listing_core::{product_listing_id::ProductListingId, title::Title};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingTranslationWrite {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    pub enrichment_event_id: EventId,
    pub source_language: Language,
    pub titles: IndexMap<Language, Title>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingTranslationWriteOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductListingNotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingTranslationWriteError {
    #[error("product translation write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingTranslationWriter: Send {
    async fn apply(
        &mut self,
        write: &ProductListingTranslationWrite,
    ) -> Result<ProductListingTranslationWriteOutcome, ProductListingTranslationWriteError>;
}

pub trait ProductListingTranslationWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingTranslationWriter + 'tx;
}
