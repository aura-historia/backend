use crate::use_cases::{
    party_location_resolution::{PartyLocationResolutionReviewsPage, ResolutionAttention},
    party_locations::PartyLocationError,
};
use party_core::party_location_id::PartyLocationId;

/// Cross-Party review queue of locations whose current resolution needs attention.
#[async_trait::async_trait]
pub trait PartyLocationResolutionReviewReader: Send {
    /// Locations in ID order after `after` whose bound resolution has one of `statuses`.
    /// Resolutions of a page are read in one batch within the caller's transaction; an item
    /// whose resolution changed status before that read is omitted, never misreported.
    async fn list(
        &mut self,
        statuses: &[ResolutionAttention],
        after: Option<PartyLocationId>,
        limit: u32,
    ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError>;
}
pub trait PartyLocationResolutionReviewReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartyLocationResolutionReviewReader + 'tx;
}
