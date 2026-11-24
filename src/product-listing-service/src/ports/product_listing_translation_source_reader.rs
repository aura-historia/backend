use application::error::BoxError;
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    product_listing_id::ProductListingId, title::Title,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingTranslationSourceEvent {
    Discovered,
    ChangedText,
    /// A change that put a listing back on sale without changing its text; it catches up the
    /// current content revision.
    ReturnedToSale,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingTranslationSource {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub content_source_event_id: EventId,
    pub event: ProductListingTranslationSourceEvent,
    /// The listing's current lifecycle and availability, not the ones the event recorded.
    pub lifecycle: ListingLifecycle,
    pub availability: Option<ListingAvailability>,
    pub title: Option<Title>,
    pub title_language: Option<Language>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingTranslationSourceReadError {
    #[error("product translation source query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product translation source persisted state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingTranslationSourceReader: Send + Sync {
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingTranslationSource>, ProductListingTranslationSourceReadError>;
}
