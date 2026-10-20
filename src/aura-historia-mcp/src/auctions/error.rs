use crate::OperationError;
use application::error::box_error;
use auction_service::use_cases::{GetPublicAuctionError, ListAuctionsError};
use product_listing_service::use_cases::GetAuctionCatalogueError;

pub(super) fn list(error: ListAuctionsError) -> OperationError {
    match error {
        error @ (ListAuctionsError::IncompleteScheduleInstantRange
        | ListAuctionsError::InvalidScheduleInstantRange
        | ListAuctionsError::ScheduledSortRoleMismatch
        | ListAuctionsError::CursorScopeMismatch) => {
            OperationError::InvalidArguments(box_error(error))
        }
        error @ ListAuctionsError::TemporarilyUnavailable { .. } => {
            OperationError::Unavailable(box_error(error))
        }
        error @ ListAuctionsError::InvalidReadModel { .. } => {
            OperationError::Internal(box_error(error))
        }
    }
}
pub(super) fn get(error: GetPublicAuctionError) -> OperationError {
    match error {
        GetPublicAuctionError::NotFound => OperationError::NotFound,
        error @ GetPublicAuctionError::TemporarilyUnavailable { .. } => {
            OperationError::Unavailable(box_error(error))
        }
        error @ GetPublicAuctionError::InvalidReadModel { .. } => {
            OperationError::Internal(box_error(error))
        }
    }
}
pub(super) fn catalogue(error: GetAuctionCatalogueError) -> OperationError {
    match error {
        GetAuctionCatalogueError::Shipping(error) => crate::shipping_error::shipping(error),
        GetAuctionCatalogueError::AuctionNotFound => OperationError::NotFound,
        error @ GetAuctionCatalogueError::CursorScopeMismatch => {
            OperationError::InvalidArguments(box_error(error))
        }
        error @ (GetAuctionCatalogueError::TemporarilyUnavailable { .. }
        | GetAuctionCatalogueError::PricingFxSnapshotMissing) => {
            OperationError::Unavailable(box_error(error))
        }
        error @ (GetAuctionCatalogueError::InvalidReadModel { .. }
        | GetAuctionCatalogueError::PricingPresentationFailed { .. }) => {
            OperationError::Internal(box_error(error))
        }
    }
}
