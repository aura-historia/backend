use application::error::BoxError;
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use product_core::{product_id::ProductId, title::Title};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductTranslationWrite {
    pub product_id: ProductId,
    pub source_event_id: EventId,
    pub enrichment_event_id: EventId,
    pub source_language: Language,
    pub titles: IndexMap<Language, Title>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductTranslationWriteOutcome {
    Applied,
    Duplicate,
    Stale,
    ProductNotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductTranslationWriteError {
    #[error("product translation write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductTranslationWriter: Send {
    async fn apply(
        &mut self,
        write: &ProductTranslationWrite,
    ) -> Result<ProductTranslationWriteOutcome, ProductTranslationWriteError>;
}

pub trait ProductTranslationWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductTranslationWriter + 'tx;
}
