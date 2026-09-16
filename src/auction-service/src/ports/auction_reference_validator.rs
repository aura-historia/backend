use application::error::BoxError;
use auction_core::AuctionId;
use listing_source_core::ListingSourceId;

/// Transaction-scoped validation for a typed ProductListing Auction reference.
///
/// This deliberately validates only referential ownership. It never returns or mutates an
/// Auction aggregate.
#[derive(Debug, thiserror::Error)]
pub enum AuctionReferenceValidationError {
    #[error("auction not found")]
    NotFound,
    #[error("auction belongs to another listing source")]
    ListingSourceMismatch,
    #[error("temporary auction reference validation failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction reference state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionReferenceValidator: Send {
    async fn validate(
        &mut self,
        auction_id: AuctionId,
        listing_source_id: ListingSourceId,
    ) -> Result<(), AuctionReferenceValidationError>;
}

pub trait AuctionReferenceValidatorFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuctionReferenceValidator + 'tx;
}
