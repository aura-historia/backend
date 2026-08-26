use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingContentAssessmentWrite {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    pub decision: Option<ContentPolicyDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingContentAssessmentWriteOutcome {
    Applied,
    Cleared,
    Duplicate,
    Stale,
    ProductListingNotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingContentAssessmentWriteError {
    #[error("product content assessment write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingContentAssessmentWriter: Send {
    async fn apply(
        &mut self,
        write: &ProductListingContentAssessmentWrite,
    ) -> Result<
        ProductListingContentAssessmentWriteOutcome,
        ProductListingContentAssessmentWriteError,
    >;
}

pub trait ProductListingContentAssessmentWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingContentAssessmentWriter + 'tx;
}
