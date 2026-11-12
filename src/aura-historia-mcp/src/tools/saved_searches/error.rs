use crate::OperationError;
use application::error::box_error;
use search_filter_service::use_cases::{
    CreateSearchFilterError, DeleteOwnedSearchFilterError, GetOwnedSearchFilterError,
    ListOwnedSearchFiltersError, ListSearchFilterMatchesError, UpdateOwnedSearchFilterError,
    UpdateSearchFilterMatchFeedbackError,
};

pub(super) fn list(error: ListOwnedSearchFiltersError) -> OperationError {
    match error {
        ListOwnedSearchFiltersError::AuthenticatedActorRequired
        | ListOwnedSearchFiltersError::ActorMayNotManageSearchFilter => OperationError::Forbidden,
        error @ (ListOwnedSearchFiltersError::SearchFilterListReadFailed { .. }
        | ListOwnedSearchFiltersError::ListingSourceReadFailed { .. }) => {
            OperationError::Unavailable(box_error(error))
        }
        error @ ListOwnedSearchFiltersError::ListingSourceStateInvalid { .. } => {
            OperationError::Internal(box_error(error))
        }
    }
}
pub(super) fn get(error: GetOwnedSearchFilterError) -> OperationError {
    match error {
        GetOwnedSearchFilterError::AuthenticatedActorRequired
        | GetOwnedSearchFilterError::ActorMayNotManageSearchFilter => OperationError::Forbidden,
        GetOwnedSearchFilterError::SearchFilterNotFound => {
            OperationError::NotFound("Saved search not found: check the userSearchFilterId.")
        }
        error @ (GetOwnedSearchFilterError::SearchFilterReadFailed { .. }
        | GetOwnedSearchFilterError::ListingSourceReadFailed { .. }) => {
            OperationError::Unavailable(box_error(error))
        }
        error @ GetOwnedSearchFilterError::ListingSourceStateInvalid { .. } => {
            OperationError::Internal(box_error(error))
        }
    }
}
pub(super) fn create(error: CreateSearchFilterError) -> OperationError {
    use CreateSearchFilterError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginTransactionFailed,
        CommitTransactionFailed, EmbeddingGenerationFailed, ListingSourceReadFailed,
        ListingSourceStateInvalid, PersistedSearchFilterStateInvalid, SearchFilterAlreadyExists,
        SearchFilterFeatureRestricted, SearchFilterInsertFailed, SearchFilterQuotaExceeded,
        SearchFilterQuotaReadFailed, ShippingDestination, UserNotFound,
        UserTierEntitlementsLockFailed,
    };
    match error {
        ShippingDestination(error) => crate::shipping_error::destination(error),
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        UserNotFound => OperationError::NotFound("Account not found: sign in again."),
        SearchFilterAlreadyExists => OperationError::conflict("Saved search already exists."),
        SearchFilterQuotaExceeded { .. } => OperationError::SearchFilterQuotaExceeded,
        SearchFilterFeatureRestricted { .. } => OperationError::SearchFilterRestrictedFeature,
        error @ (PersistedSearchFilterStateInvalid { .. } | ListingSourceStateInvalid { .. }) => {
            OperationError::Internal(box_error(error))
        }
        error @ (EmbeddingGenerationFailed { .. }
        | ListingSourceReadFailed { .. }
        | UserTierEntitlementsLockFailed { .. }
        | SearchFilterQuotaReadFailed { .. }
        | SearchFilterInsertFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(box_error(error)),
    }
}
pub(super) fn update(error: UpdateOwnedSearchFilterError) -> OperationError {
    use UpdateOwnedSearchFilterError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginTransactionFailed,
        CommitTransactionFailed, EmbeddingGenerationFailed, InvalidSearchFilterPatch,
        ListingSourceReadFailed, ListingSourceStateInvalid, PersistedSearchFilterStateInvalid,
        SearchFilterConcurrencyConflict, SearchFilterFeatureRestricted, SearchFilterLookupFailed,
        SearchFilterNotFound, SearchFilterQuotaExceeded, SearchFilterQuotaReadFailed,
        SearchFilterUpdateFailed, ShippingDestination, UserNotFound,
        UserTierEntitlementsLockFailed,
    };
    match error {
        ShippingDestination(error) => crate::shipping_error::destination(error),
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        UserNotFound => OperationError::NotFound("Account not found: sign in again."),
        SearchFilterNotFound => {
            OperationError::NotFound("Saved search not found: check the userSearchFilterId.")
        }
        InvalidSearchFilterPatch => OperationError::InvalidInput,
        SearchFilterQuotaExceeded { .. } => OperationError::SearchFilterQuotaExceeded,
        SearchFilterFeatureRestricted { .. } => OperationError::SearchFilterRestrictedFeature,
        SearchFilterConcurrencyConflict => {
            OperationError::conflict("Saved search changed concurrently: re-read it and retry.")
        }
        error @ (PersistedSearchFilterStateInvalid { .. } | ListingSourceStateInvalid { .. }) => {
            OperationError::Internal(box_error(error))
        }
        error @ (EmbeddingGenerationFailed { .. }
        | ListingSourceReadFailed { .. }
        | UserTierEntitlementsLockFailed { .. }
        | SearchFilterQuotaReadFailed { .. }
        | SearchFilterLookupFailed { .. }
        | SearchFilterUpdateFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(box_error(error)),
    }
}
pub(super) fn delete(error: DeleteOwnedSearchFilterError) -> OperationError {
    use DeleteOwnedSearchFilterError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginTransactionFailed,
        CommitTransactionFailed, PersistedSearchFilterStateInvalid, SearchFilterDeletionFailed,
        SearchFilterLookupFailed, SearchFilterNotFound,
    };
    match error {
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        SearchFilterNotFound => {
            OperationError::NotFound("Saved search not found: check the userSearchFilterId.")
        }
        error @ PersistedSearchFilterStateInvalid { .. } => {
            OperationError::Internal(box_error(error))
        }
        error @ (SearchFilterLookupFailed { .. }
        | SearchFilterDeletionFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(box_error(error)),
    }
}
pub(super) fn matches(error: ListSearchFilterMatchesError) -> OperationError {
    use ListSearchFilterMatchesError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginPricingTransactionFailed,
        CommitPricingTransactionFailed, CurrentPricingFxSnapshotMissing,
        HiddenProductListingRedactionFailed, MatchedProductListingMissing,
        PricingFxSnapshotInvalid, PricingFxSnapshotUnavailable, ProductListingDetailsInvalid,
        ProductListingDetailsReadFailed, ProductListingPriceConversionFailed,
        SaleFxSnapshotMismatch, SalePricingFxSnapshotMissing, SearchFilterMatchReadFailed,
        SearchFilterNotFound, Shipping,
    };
    match error {
        Shipping(error) => crate::shipping_error::shipping(error),
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        SearchFilterNotFound => {
            OperationError::NotFound("Saved search not found: check the userSearchFilterId.")
        }
        error @ (ProductListingDetailsInvalid { .. }
        | MatchedProductListingMissing { .. }
        | PricingFxSnapshotInvalid { .. }
        | SaleFxSnapshotMismatch { .. }
        | ProductListingPriceConversionFailed { .. }
        | HiddenProductListingRedactionFailed { .. }) => OperationError::Internal(box_error(error)),
        error @ (SearchFilterMatchReadFailed { .. }
        | ProductListingDetailsReadFailed { .. }
        | CurrentPricingFxSnapshotMissing
        | SalePricingFxSnapshotMissing { .. }
        | PricingFxSnapshotUnavailable { .. }
        | BeginPricingTransactionFailed(_)
        | CommitPricingTransactionFailed(_)) => OperationError::Unavailable(box_error(error)),
    }
}
pub(super) fn feedback(error: UpdateSearchFilterMatchFeedbackError) -> OperationError {
    use UpdateSearchFilterMatchFeedbackError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginTransactionFailed,
        CommitTransactionFailed, PersistedSearchFilterMatchStateInvalid,
        PersistedSearchFilterStateInvalid, SearchFilterLookupFailed, SearchFilterMatchLookupFailed,
        SearchFilterMatchNotFound, SearchFilterMatchUpdateFailed, SearchFilterNotFound,
    };
    match error {
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        SearchFilterNotFound => {
            OperationError::NotFound("Saved search not found: check the userSearchFilterId.")
        }
        SearchFilterMatchNotFound => OperationError::NotFound(
            "Match not found: the listing is not currently a match of this saved search.",
        ),
        error @ (PersistedSearchFilterStateInvalid { .. }
        | PersistedSearchFilterMatchStateInvalid { .. }) => {
            OperationError::Internal(box_error(error))
        }
        error @ (SearchFilterLookupFailed { .. }
        | SearchFilterMatchLookupFailed { .. }
        | SearchFilterMatchUpdateFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(box_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_name_the_saved_search_or_match_that_was_not_found() {
        let filter = get(GetOwnedSearchFilterError::SearchFilterNotFound).tool_error_texts();
        assert_eq!("NOT_FOUND", filter[0]);
        assert!(filter[1].contains("userSearchFilterId"), "{filter:?}");

        let missing_match =
            feedback(UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound)
                .tool_error_texts();
        assert_eq!("NOT_FOUND", missing_match[0]);
        assert!(
            missing_match[1].starts_with("Match not found"),
            "{missing_match:?}"
        );
    }

    #[test]
    fn should_say_to_reread_a_saved_search_that_changed_concurrently() {
        let texts = update(UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict)
            .tool_error_texts();
        assert_eq!("CONFLICT", texts[0]);
        assert!(texts[1].contains("re-read"), "{texts:?}");
    }
}
