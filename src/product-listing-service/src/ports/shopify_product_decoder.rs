use application::error::BoxError;
use listing_source_service::ports::ShopifySource;
use product_listing_normalization::{ProductListingNormalizationInput, SourcePayload};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopifyProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShopifyListingAction {
    Capture(ShopifyRawObservation),
    Ignore,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopifyRawObservation {
    pub source_record_key: String,
    pub input: ProductListingNormalizationInput,
    pub source_occurred_at: Option<OffsetDateTime>,
}

pub trait ShopifyProductDecoder: Send + Sync {
    fn decode(
        &self,
        kind: ShopifyProductEventKind,
        source: &ShopifySource,
        payload: SourcePayload,
    ) -> Result<ShopifyListingAction, BoxError>;
}
