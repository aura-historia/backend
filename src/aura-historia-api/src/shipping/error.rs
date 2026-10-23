//! HTTP classification of shipping management failures.
use super::bad;
use crate::error::{
    ApiError, FORBIDDEN, INVALID_CREDENTIALS, LISTING_SOURCE_NOT_FOUND, PARTY_NOT_FOUND,
    SHIPPING_CREATION_KEY_CONFLICT, SHIPPING_LOCATION_NOT_FOUND, SHIPPING_PROFILE_NOT_FOUND,
    SHIPPING_PROFILE_RETIRED, SHIPPING_REFERENCE_INELIGIBLE, SHIPPING_REVISION_EXHAUSTED,
    SHIPPING_STALE_REVISION, SHIPPING_STORAGE_ERROR, SHIPPING_TEMPORARILY_UNAVAILABLE,
};
use shipping_service::ShippingError;

impl From<ShippingError> for ApiError {
    fn from(e: ShippingError) -> Self {
        match e {
            ShippingError::AuthenticationRequired => Self::unauthorized(INVALID_CREDENTIALS),
            ShippingError::SourceAccessDenied => Self::forbidden(FORBIDDEN),
            ShippingError::SourceNotFound => Self::not_found(LISTING_SOURCE_NOT_FOUND),
            ShippingError::ProfileNotFound => Self::not_found(SHIPPING_PROFILE_NOT_FOUND),
            ShippingError::AttributedPartyNotFound { .. } => Self::not_found(PARTY_NOT_FOUND),
            ShippingError::LocationNotFound => Self::not_found(SHIPPING_LOCATION_NOT_FOUND),
            ShippingError::InvalidInput
            | ShippingError::InvalidRevision
            | ShippingError::InvalidTerms(shipping_core::InvalidShippingValue::SourceMismatch) => {
                bad()
            }
            ShippingError::InvalidTerms(shipping_core::InvalidShippingValue::RetiredProfile)
            | ShippingError::LocationRetired => Self::conflict(SHIPPING_REFERENCE_INELIGIBLE),
            ShippingError::InvalidTerms(_)
            | ShippingError::Profile(shipping_core::profile::ShippingProfileError::Invalid(_)) => {
                bad()
            }
            ShippingError::ImportAuthorityStale => {
                Self::conflict(crate::error::SHIPPING_IMPORT_AUTHORITY_STALE)
            }
            ShippingError::ImportOwnershipConflict => {
                Self::conflict(crate::error::SHIPPING_IMPORT_OWNERSHIP_CONFLICT)
            }
            ShippingError::ImportRequestConflict => {
                Self::conflict(crate::error::SHIPPING_IMPORT_REQUEST_CONFLICT)
            }
            ShippingError::ImportOutOfOrder => {
                Self::conflict(crate::error::SHIPPING_IMPORT_OUT_OF_ORDER)
            }
            ShippingError::ImportIdentityExists => {
                Self::conflict(crate::error::SHIPPING_IMPORT_IDENTITY_EXISTS)
            }
            ShippingError::ImportDependencyMissing => {
                Self::not_found(crate::error::SHIPPING_IMPORT_DEPENDENCY_MISSING)
            }
            ShippingError::StaleRevision => Self::conflict(SHIPPING_STALE_REVISION),
            ShippingError::CreationKeyConflict => Self::conflict(SHIPPING_CREATION_KEY_CONFLICT),
            ShippingError::RevisionExhausted
            | ShippingError::Profile(
                shipping_core::profile::ShippingProfileError::RevisionExhausted,
            ) => Self::conflict(SHIPPING_REVISION_EXHAUSTED),
            ShippingError::Profile(shipping_core::profile::ShippingProfileError::Retired) => {
                Self::conflict(SHIPPING_PROFILE_RETIRED)
            }
            ShippingError::PersistenceUnavailable { .. } | ShippingError::Transaction(_) => {
                Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
            }
            ShippingError::CorruptShippingState { .. }
            | ShippingError::PersistenceFailed { .. } => {
                Self::internal_server_error(SHIPPING_STORAGE_ERROR)
            }
        }
    }
}
