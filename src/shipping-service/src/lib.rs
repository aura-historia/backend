pub mod assessment;
pub mod ports;
pub mod use_cases;

use application::{error::BoxError, transaction::TransactionError};
use listing_source_service::use_cases::location_default::LocationDefaultError;

#[derive(Debug, thiserror::Error)]
pub enum ShippingError {
    #[error("authenticated shipping manager required")]
    AuthenticationRequired,
    #[error("caller lacks shipping management authority")]
    SourceAccessDenied,
    #[error("listing source not found")]
    SourceNotFound,
    #[error("shipping profile not found in this source")]
    ProfileNotFound,
    #[error("attributed party not found")]
    AttributedPartyNotFound {
        #[source]
        source: BoxError,
    },
    #[error("referenced location is absent or belongs to another operator")]
    LocationNotFound,
    #[error("referenced location is retired")]
    LocationRetired,
    #[error("shipping management revision is stale")]
    StaleRevision,
    #[error("shipping management revision is invalid")]
    InvalidRevision,
    #[error("shipping management revision exhausted")]
    RevisionExhausted,
    #[error("creation key has already been used for different content")]
    CreationKeyConflict,
    #[error("invalid shipping management input")]
    InvalidInput,
    #[error("invalid shipping terms")]
    InvalidTerms(
        #[from]
        #[source]
        shipping_core::InvalidShippingValue,
    ),
    #[error("shipping profile change rejected")]
    Profile(
        #[from]
        #[source]
        shipping_core::profile::ShippingProfileError,
    ),
    #[error("shipping persistence is temporarily unavailable")]
    PersistenceUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("shipping storage cannot be decoded")]
    CorruptShippingState {
        #[source]
        source: BoxError,
    },
    #[error("shipping storage operation failed")]
    PersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("shipping transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}

impl From<LocationDefaultError> for ShippingError {
    fn from(error: LocationDefaultError) -> Self {
        match error {
            LocationDefaultError::AuthenticationRequired => Self::AuthenticationRequired,
            LocationDefaultError::Forbidden => Self::SourceAccessDenied,
            LocationDefaultError::SourceNotFound => Self::SourceNotFound,
            LocationDefaultError::LocationNotFound => Self::LocationNotFound,
            LocationDefaultError::LocationRetired => Self::LocationRetired,
            LocationDefaultError::ConcurrencyConflict => Self::StaleRevision,
            LocationDefaultError::RevisionExhausted => Self::RevisionExhausted,
            LocationDefaultError::InvalidRevision => Self::InvalidRevision,
            LocationDefaultError::TemporarilyUnavailable { source } => {
                Self::PersistenceUnavailable { source }
            }
            LocationDefaultError::InvalidPersistedState { source } => {
                Self::CorruptShippingState { source }
            }
            LocationDefaultError::Internal { source } => Self::PersistenceFailed { source },
            LocationDefaultError::Transaction(source) => Self::Transaction(source),
            other => Self::PersistenceFailed {
                source: Box::new(other),
            },
        }
    }
}

pub mod projection;
