use application::error::BoxError;
use domain_primitives::event_id::EventId;
use product_listing_core::{
    description::Description, product_listing_id::ProductListingId, title::Title,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingContentAssessmentSource {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub current_event_id: EventId,
    pub event_group: String,
    pub event_type: String,
    pub title: Option<Title>,
    pub description: Option<Description>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingContentAssessmentSourceReadError {
    #[error("product content assessment source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product content assessment source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingContentAssessmentSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        Option<ProductListingContentAssessmentSource>,
        ProductListingContentAssessmentSourceReadError,
    >;
}
