use crate::{OperationError, shipping_error::shipping};
use product_listing_service::use_cases::{
    GetProductListingError, GetSimilarProductListingsError, SearchProductListingsError,
};

pub(crate) fn details(error: GetProductListingError) -> OperationError {
    use GetProductListingError::*;
    match error {
        Shipping(error) => shipping(error),
        NotFound => OperationError::NotFound,
        error @ (ProductListingDetailsQueryFailed
        | PricingFxSnapshotMissing
        | PricingFxSnapshotUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ (ProductListingDetailsReadModelInvalid
        | PricingFxSnapshotInvalid { .. }
        | SaleObservationFxSnapshotMismatch { .. }
        | ProductListingPriceConversionFailed { .. }) => OperationError::Internal(Box::new(error)),
    }
}
pub(super) fn search(error: SearchProductListingsError) -> OperationError {
    use SearchProductListingsError::*;
    match error {
        Shipping(error) => shipping(error),
        error @ (InvalidCursor | InvalidGeographicQuery) => {
            OperationError::InvalidArguments(Box::new(error))
        }
        error @ (ProductListingSearchQueryFailed
        | QueryEmbeddingFailed { .. }
        | FxRateSnapshotMissing
        | FxRateSnapshotReadFailed { .. }
        | ListingSourceSummaryQueryFailed { .. }
        | ProductListingUserStateQueryFailed { .. }
        | ContentAssessmentQueryFailed { .. }) => OperationError::Unavailable(Box::new(error)),
        error @ (ProductListingSearchReadModelInvalid
        | FxRateSnapshotInvalid { .. }
        | ListingSourceSummaryReadModelInvalid { .. }
        | ListingSourceSummaryMissing { .. }
        | ProductListingUserStateReadModelInvalid { .. }
        | ProductListingUserStateMissing
        | HiddenProductListingSummaryInvalid { .. }
        | ContentAssessmentStateInvalid { .. }) => OperationError::Internal(Box::new(error)),
    }
}
pub(super) fn similar(error: GetSimilarProductListingsError) -> OperationError {
    use GetSimilarProductListingsError::*;
    match error {
        Shipping(error) => shipping(error),
        NotFound => OperationError::NotFound,
        error @ (ProductListingEmbeddingQueryFailed { .. }
        | SimilaritySearchUnavailable
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)
        | PricingFxSnapshotMissing
        | PricingFxSnapshotUnavailable { .. }
        | ListingSourceSummaryQueryFailed { .. }
        | ProductListingUserStateQueryFailed { .. }
        | ContentAssessmentQueryFailed { .. }) => OperationError::Unavailable(Box::new(error)),
        error @ (PricingFxSnapshotInvalid { .. }
        | ListingSourceSummaryReadModelInvalid { .. }
        | ListingSourceSummaryMissing { .. }
        | ProductListingUserStateReadModelInvalid { .. }
        | ProductListingUserStateMissing
        | HiddenProductListingSummaryInvalid { .. }
        | ContentAssessmentStateInvalid { .. }) => OperationError::Internal(Box::new(error)),
    }
}
