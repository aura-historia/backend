use crate::{OperationError, product_listings::error::details, shipping_error::shipping};
use watchlist_service::use_cases::{
    ListWatchlistError, UnwatchProductListingError, UpdateWatchlistProductListingError,
    WatchProductListingError,
};

pub(super) fn list(error: ListWatchlistError) -> OperationError {
    use ListWatchlistError::*;
    match error {
        Shipping(error) => shipping(error),
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        error @ (TemporarilyUnavailable
        | CurrentPricingFxSnapshotMissing
        | SalePricingFxSnapshotMissing { .. }
        | PricingFxSnapshotUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ (InvalidPersistedState
        | PricingFxSnapshotInvalid { .. }
        | SaleFxSnapshotMismatch { .. }
        | ProductListingPriceConversionFailed { .. }) => OperationError::Internal(Box::new(error)),
    }
}

pub(super) fn add(error: WatchProductListingError) -> OperationError {
    use WatchProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        UserNotFound | ProductListingNotFound => OperationError::NotFound,
        AlreadyExists | ProductListingUnavailable => OperationError::Conflict(None),
        WatchlistQuotaExceeded { .. } => OperationError::WatchlistQuotaExceeded,
        error @ (TemporarilyUnavailable { .. }
        | UserTierEntitlementsLockFailed { .. }
        | WatchlistQuotaReadFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ InvalidPersistedState => OperationError::Internal(Box::new(error)),
    }
}

pub(super) fn remove(error: UnwatchProductListingError) -> OperationError {
    use UnwatchProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        NotFound => OperationError::NotFound,
        ConcurrencyConflict => OperationError::Conflict(None),
        error @ (TemporarilyUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ InvalidPersistedState => OperationError::Internal(Box::new(error)),
    }
}

pub(super) fn update(error: UpdateWatchlistProductListingError) -> OperationError {
    use UpdateWatchlistProductListingError::*;
    match error {
        Presentation(error) => details(error),
        AuthenticatedActorRequired | Forbidden => OperationError::Forbidden,
        NotFound | UserNotFound | ProductListingNotFound => OperationError::NotFound,
        ConcurrencyConflict | ProductListingUnavailable => OperationError::Conflict(None),
        WatchlistQuotaExceeded { .. } => OperationError::WatchlistQuotaExceeded,
        error @ (TemporarilyUnavailable { .. }
        | UserTierEntitlementsLockFailed { .. }
        | WatchlistQuotaReadFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ InvalidPersistedState => OperationError::Internal(Box::new(error)),
    }
}
