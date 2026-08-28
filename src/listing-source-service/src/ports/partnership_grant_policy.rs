use application::operation_context::Principal;
use party_core::party_id::PartyId;

use super::ListingSourceReadError;

#[async_trait::async_trait]
pub trait PartnershipGrantPolicy: Send + Sync {
    async fn can_access_source(
        &self,
        principal: &Principal,
        operator_party_id: PartyId,
    ) -> Result<bool, ListingSourceReadError>;
}
