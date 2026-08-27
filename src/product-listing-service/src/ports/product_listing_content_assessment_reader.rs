use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingContentAssessment {
    pub product_listing_id: ProductListingId,
    pub source_event_id: EventId,
    pub decision: ContentPolicyDecision,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingContentAssessmentReadError {
    #[error("product content assessment query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product content assessment persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingContentAssessmentReader: Send + Sync {
    async fn find_current_assessments(
        &self,
        product_listing_ids: &[ProductListingId],
    ) -> Result<
        HashMap<ProductListingId, ProductListingContentAssessment>,
        ProductListingContentAssessmentReadError,
    >;
}
