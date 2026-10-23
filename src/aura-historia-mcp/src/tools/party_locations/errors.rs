use crate::OperationError;
use party_service::use_cases::party_locations::PartyLocationError;
pub(super) fn location(error: PartyLocationError) -> OperationError {
    match error {
        PartyLocationError::AuthenticationRequired | PartyLocationError::Forbidden => {
            OperationError::Forbidden
        }
        PartyLocationError::NotFound => OperationError::NotFound,
        PartyLocationError::ConcurrencyConflict
        | PartyLocationError::RevisionExhausted
        | PartyLocationError::Domain(_) => OperationError::Conflict,
        PartyLocationError::InvalidInput => OperationError::InvalidInput,
        PartyLocationError::TemporarilyUnavailable { source } => {
            OperationError::Unavailable(source)
        }
        PartyLocationError::Transaction(error) => OperationError::Unavailable(Box::new(error)),
        PartyLocationError::InvalidPersistedState { source }
        | PartyLocationError::Internal { source } => OperationError::Internal(source),
    }
}
