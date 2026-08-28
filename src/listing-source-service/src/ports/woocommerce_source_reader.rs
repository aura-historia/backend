use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use party_core::party_id::PartyId;

use super::ListingSourceReadError;

#[derive(Debug, Clone, PartialEq)]
pub struct WoocommerceSource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[async_trait::async_trait]
pub trait WoocommerceSourceReader: Send + Sync {
    async fn find_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<WoocommerceSource>, ListingSourceReadError>;
}
