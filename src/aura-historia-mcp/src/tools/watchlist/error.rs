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
        UserNotFound => OperationError::NotFound("Account not found: sign in again."),
        ProductListingNotFound => {
            OperationError::NotFound("Listing not found: check the productListingId.")
        }
        AlreadyExists => OperationError::conflict("Listing is already on the watchlist."),
        ProductListingUnavailable => {
            OperationError::conflict("Listing is no longer available and cannot be watched.")
        }
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
        NotFound => {
            OperationError::NotFound("Listing is not on the watchlist: check the productListingId.")
        }
        ConcurrencyConflict => {
            OperationError::conflict("Watchlist entry changed concurrently: retry the request.")
        }
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
        NotFound | ProductListingNotFound => {
            OperationError::NotFound("Listing is not on the watchlist: check the productListingId.")
        }
        UserNotFound => OperationError::NotFound("Account not found: sign in again."),
        ConcurrencyConflict => {
            OperationError::conflict("Watchlist entry changed concurrently: retry the request.")
        }
        ProductListingUnavailable => OperationError::conflict(
            "Listing is no longer available; its watchlist entry cannot be updated.",
        ),
        WatchlistQuotaExceeded { .. } => OperationError::WatchlistQuotaExceeded,
        error @ (TemporarilyUnavailable { .. }
        | UserTierEntitlementsLockFailed { .. }
        | WatchlistQuotaReadFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ InvalidPersistedState => OperationError::Internal(Box::new(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_tell_a_duplicate_add_from_a_missing_listing() {
        assert_eq!(
            vec!["CONFLICT", "Listing is already on the watchlist."],
            add(WatchProductListingError::AlreadyExists).tool_error_texts()
        );
        let missing = add(WatchProductListingError::ProductListingNotFound).tool_error_texts();
        assert_eq!("NOT_FOUND", missing[0]);
        assert!(missing[1].contains("productListingId"), "{missing:?}");
    }

    #[test]
    fn should_name_the_watchlist_entry_when_removing_or_updating_a_missing_one() {
        for texts in [
            remove(UnwatchProductListingError::NotFound).tool_error_texts(),
            update(UpdateWatchlistProductListingError::NotFound).tool_error_texts(),
        ] {
            assert_eq!("NOT_FOUND", texts[0]);
            assert!(
                texts[1].starts_with("Listing is not on the watchlist"),
                "{texts:?}"
            );
        }
    }

    #[test]
    fn should_say_to_retry_after_a_concurrent_watchlist_change() {
        for texts in [
            remove(UnwatchProductListingError::ConcurrencyConflict).tool_error_texts(),
            update(UpdateWatchlistProductListingError::ConcurrencyConflict).tool_error_texts(),
        ] {
            assert_eq!("CONFLICT", texts[0]);
            assert!(texts[1].contains("retry"), "{texts:?}");
        }
    }
}
