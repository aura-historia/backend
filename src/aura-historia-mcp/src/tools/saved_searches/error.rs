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
        error @ ListOwnedSearchFiltersError::SearchFilterListReadFailed { .. } => {
            OperationError::Unavailable(box_error(error))
        }
    }
}
pub(super) fn get(error: GetOwnedSearchFilterError) -> OperationError {
    match error {
        GetOwnedSearchFilterError::AuthenticatedActorRequired
        | GetOwnedSearchFilterError::ActorMayNotManageSearchFilter => OperationError::Forbidden,
        GetOwnedSearchFilterError::SearchFilterNotFound => OperationError::NotFound,
        error @ GetOwnedSearchFilterError::SearchFilterReadFailed { .. } => {
            OperationError::Unavailable(box_error(error))
        }
    }
}
pub(super) fn create(error: CreateSearchFilterError) -> OperationError {
    use CreateSearchFilterError::{
        ActorMayNotManageSearchFilter, AuthenticatedActorRequired, BeginTransactionFailed,
        CommitTransactionFailed, EmbeddingGenerationFailed, PersistedSearchFilterStateInvalid,
        SearchFilterAlreadyExists, SearchFilterFeatureRestricted, SearchFilterInsertFailed,
        SearchFilterQuotaExceeded, SearchFilterQuotaReadFailed, ShippingDestination, UserNotFound,
        UserTierEntitlementsLockFailed,
    };
    match error {
        ShippingDestination(error) => crate::shipping_error::destination(error),
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        UserNotFound => OperationError::NotFound,
        SearchFilterAlreadyExists => OperationError::Conflict(None),
        SearchFilterQuotaExceeded { .. } => OperationError::SearchFilterQuotaExceeded,
        SearchFilterFeatureRestricted { .. } => OperationError::SearchFilterRestrictedFeature,
        error @ PersistedSearchFilterStateInvalid { .. } => {
            OperationError::Internal(box_error(error))
        }
        error @ (EmbeddingGenerationFailed { .. }
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
        PersistedSearchFilterStateInvalid, SearchFilterConcurrencyConflict,
        SearchFilterFeatureRestricted, SearchFilterLookupFailed, SearchFilterNotFound,
        SearchFilterQuotaExceeded, SearchFilterQuotaReadFailed, SearchFilterUpdateFailed,
        ShippingDestination, UserNotFound, UserTierEntitlementsLockFailed,
    };
    match error {
        ShippingDestination(error) => crate::shipping_error::destination(error),
        AuthenticatedActorRequired | ActorMayNotManageSearchFilter => OperationError::Forbidden,
        UserNotFound | SearchFilterNotFound => OperationError::NotFound,
        InvalidSearchFilterPatch => OperationError::InvalidInput,
        SearchFilterQuotaExceeded { .. } => OperationError::SearchFilterQuotaExceeded,
        SearchFilterFeatureRestricted { .. } => OperationError::SearchFilterRestrictedFeature,
        SearchFilterConcurrencyConflict => OperationError::Conflict(None),
        error @ PersistedSearchFilterStateInvalid { .. } => {
            OperationError::Internal(box_error(error))
        }
        error @ (EmbeddingGenerationFailed { .. }
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
        SearchFilterNotFound => OperationError::NotFound,
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
        SearchFilterNotFound => OperationError::NotFound,
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
        SearchFilterNotFound | SearchFilterMatchNotFound => OperationError::NotFound,
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
