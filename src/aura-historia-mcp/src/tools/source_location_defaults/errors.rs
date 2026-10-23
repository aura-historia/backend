use crate::OperationError;
use listing_source_service::use_cases::location_default::LocationDefaultError;
pub(super) fn default(error: LocationDefaultError) -> OperationError {
    match error {
        LocationDefaultError::AuthenticationRequired | LocationDefaultError::Forbidden => {
            OperationError::Forbidden
        }
        LocationDefaultError::SourceNotFound | LocationDefaultError::LocationNotFound => {
            OperationError::NotFound
        }
        LocationDefaultError::LocationRetired
        | LocationDefaultError::DefaultNotAssigned
        | LocationDefaultError::ConcurrencyConflict
        | LocationDefaultError::RevisionExhausted => OperationError::Conflict,
        LocationDefaultError::InvalidRevision | LocationDefaultError::InvalidAssignment(_) => {
            OperationError::InvalidInput
        }
        LocationDefaultError::TemporarilyUnavailable { source } => {
            OperationError::Unavailable(source)
        }
        LocationDefaultError::Transaction(error) => OperationError::Unavailable(Box::new(error)),
        LocationDefaultError::InvalidPersistedState { source }
        | LocationDefaultError::Internal { source } => OperationError::Internal(source),
    }
}
