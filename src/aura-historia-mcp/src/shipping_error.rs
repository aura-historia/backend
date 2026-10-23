use crate::OperationError;
use product_listing_service::shipping::ShippingReadError;

pub(crate) fn shipping(error: ShippingReadError) -> OperationError {
    use shipping_service::assessment::AssessmentError;
    match error {
        ShippingReadError::Assessment(AssessmentError::Forbidden) => OperationError::Forbidden,
        ShippingReadError::Assessment(AssessmentError::Destination(error)) => destination(error),
        error @ (ShippingReadError::FxSnapshotMissing
        | ShippingReadError::FxRead(
            fxrate_service::ports::FxRateSnapshotReadError::ReadFailed { .. },
        )
        | ShippingReadError::Assessment(
            AssessmentError::Storage(_) | AssessmentError::Transaction(_),
        )) => OperationError::Unavailable(Box::new(error)),
        error => OperationError::Internal(Box::new(error)),
    }
}
pub(crate) fn destination(error: user_service::address_book::AddressBookError) -> OperationError {
    use geo_service::resolution::ResolutionError;
    use user_service::address_book::AddressBookError::*;
    match error {
        AuthenticationRequired | Forbidden | Resolution(ResolutionError::Forbidden) => {
            OperationError::Forbidden
        }
        NotFound | Resolution(ResolutionError::Missing) => OperationError::NotFound,
        Conflict | Resolution(ResolutionError::Conflict) => OperationError::Conflict(None),
        InvalidInput | Resolution(ResolutionError::InvalidInput) => OperationError::InvalidInput,
        error @ (Storage(_)
        | Transaction(_)
        | Resolution(ResolutionError::Storage(_) | ResolutionError::Transaction(_))) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (InvalidState(_)
        | Resolution(ResolutionError::Provider(_) | ResolutionError::InvalidState(_))) => {
            OperationError::Internal(Box::new(error))
        }
    }
}
