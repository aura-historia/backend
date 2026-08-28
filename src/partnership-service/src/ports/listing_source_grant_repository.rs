use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;

use super::PartnershipGrantError;

#[async_trait::async_trait]
pub trait ListingSourceGrantRepository: Send {
    async fn grant_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<(), PartnershipGrantError>;
}

pub trait ListingSourceGrantRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceGrantRepository + 'tx;
}
