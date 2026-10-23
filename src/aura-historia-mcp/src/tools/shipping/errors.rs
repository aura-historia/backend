//! Safe classification retains technical causes without exposing them in tool results.
use crate::OperationError;
use shipping_core::{InvalidShippingValue, profile::ShippingProfileError};
use shipping_service::ShippingError;
pub(super) fn shipping(error: ShippingError) -> OperationError {
    match error {
        ShippingError::AuthenticationRequired | ShippingError::SourceAccessDenied => {
            OperationError::Forbidden
        }
        ShippingError::SourceNotFound
        | ShippingError::ProfileNotFound
        | ShippingError::AttributedPartyNotFound { .. }
        | ShippingError::LocationNotFound
        | ShippingError::ImportDependencyMissing => OperationError::NotFound,
        ShippingError::InvalidInput
        | ShippingError::InvalidRevision
        | ShippingError::InvalidTerms(InvalidShippingValue::SourceMismatch)
        | ShippingError::Profile(ShippingProfileError::Invalid(_)) => OperationError::InvalidInput,
        ShippingError::InvalidTerms(InvalidShippingValue::RetiredProfile)
        | ShippingError::LocationRetired
        | ShippingError::ImportAuthorityStale
        | ShippingError::ImportOwnershipConflict
        | ShippingError::ImportRequestConflict
        | ShippingError::ImportOutOfOrder
        | ShippingError::ImportIdentityExists
        | ShippingError::StaleRevision
        | ShippingError::CreationKeyConflict
        | ShippingError::RevisionExhausted
        | ShippingError::Profile(
            ShippingProfileError::RevisionExhausted | ShippingProfileError::Retired,
        ) => OperationError::Conflict(None),
        ShippingError::InvalidTerms(_) => OperationError::InvalidInput,
        error @ (ShippingError::PersistenceUnavailable { .. } | ShippingError::Transaction(_)) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (ShippingError::CorruptShippingState { .. }
        | ShippingError::PersistenceFailed { .. }) => OperationError::Internal(Box::new(error)),
    }
}
