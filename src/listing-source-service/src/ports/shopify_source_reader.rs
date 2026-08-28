use listing_source_core::{Domain, ListingSourceId};
use localization::Language;
use money::Currency;
use party_core::party_id::PartyId;

use super::ListingSourceReadError;

#[derive(Debug, Clone, PartialEq)]
pub struct ShopifySource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub domain: Domain,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[async_trait::async_trait]
pub trait ShopifySourceReader: Send + Sync {
    async fn find_by_domain(
        &self,
        domain: &Domain,
    ) -> Result<Option<ShopifySource>, ListingSourceReadError>;
}
