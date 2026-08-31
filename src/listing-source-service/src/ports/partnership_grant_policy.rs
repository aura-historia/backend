use application::operation_context::Principal;
use listing_source_core::ListingSourceId;

use super::ListingSourceReadError;

#[async_trait::async_trait]
pub trait PartnershipGrantPolicy: Send + Sync {
    async fn can_access_source(
        &self,
        principal: &Principal,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, ListingSourceReadError>;
}
