use crate::OperationError;
use partnership_service::use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesError;
use product_listing_service::use_cases::{
    admin_product_listing_error::AdminProductListingError,
    partner_product_listing_error::PartnerProductListingError,
};
pub(super) fn sources(error: ListAdministeredListingSourcesError) -> OperationError {
    use ListAdministeredListingSourcesError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        error @ (InvalidPageSize | CursorScopeMismatch) => {
            OperationError::InvalidArguments(Box::new(error))
        }
        error @ TemporarilyUnavailable { .. } => OperationError::Unavailable(Box::new(error)),
        error @ (InvalidReadModel { .. } | Internal { .. }) => {
            OperationError::Internal(Box::new(error))
        }
    }
}
pub(super) fn management(error: PartnerProductListingError) -> OperationError {
    use PartnerProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        Management(error) => admin(error),
    }
}
fn admin(error: AdminProductListingError) -> OperationError {
    use AdminProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        NotFound | AuctionNotFound => OperationError::NotFound,
        ConcurrencyConflict | ListingWithdrawn | AuctionSourceMismatch => {
            OperationError::Conflict(None)
        }
        error @ (InvalidPatch { .. } | InvalidPageSize | InvalidQuery | CursorScopeMismatch) => {
            OperationError::InvalidArguments(Box::new(error))
        }
        error @ (TemporarilyUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ (InvalidPersistedState { .. } | Internal { .. }) => {
            OperationError::Internal(Box::new(error))
        }
    }
}
