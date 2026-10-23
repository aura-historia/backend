use crate::OperationError;
use product_listing_service::use_cases::ProductListingDeliveryError;

pub(super) fn delivery(error: ProductListingDeliveryError) -> OperationError {
    use ProductListingDeliveryError::*;
    match error {
        AuthenticationRequired | Forbidden => OperationError::Forbidden,
        SourceNotFound | NotFound => OperationError::NotFound,
        StaleRevision | ListingWithdrawn | IntegrationMismatch | ReferenceIneligible => {
            OperationError::Conflict(None)
        }
        InvalidInput { source } => OperationError::InvalidArguments(source),
        Import(error) => super::super::shipping::errors::shipping(error),
        error @ (Unavailable | Transaction(_)) => OperationError::Unavailable(Box::new(error)),
        error @ OperationFailed { .. } => OperationError::Internal(Box::new(error)),
    }
}
