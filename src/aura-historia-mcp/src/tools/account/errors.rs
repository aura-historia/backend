use crate::OperationError;
use geo_service::resolution::ResolutionError;
use user_service::{
    address_book::AddressBookError,
    use_cases::{
        commands::update_user_profile::UpdateUserProfileError,
        queries::get_own_user::GetOwnUserError,
    },
};

pub(super) fn get_profile(error: GetOwnUserError) -> OperationError {
    match error {
        GetOwnUserError::AuthenticatedActorRequired | GetOwnUserError::Forbidden => {
            OperationError::Forbidden
        }
        GetOwnUserError::NotFound => OperationError::NotFound,
        error @ (GetOwnUserError::TemporarilyUnavailable { .. }
        | GetOwnUserError::BeginTransactionFailed(_)
        | GetOwnUserError::CommitTransactionFailed(_)) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (GetOwnUserError::InvalidReadModel { .. } | GetOwnUserError::Internal { .. }) => {
            OperationError::Internal(Box::new(error))
        }
    }
}

pub(super) fn update_profile(error: UpdateUserProfileError) -> OperationError {
    match error {
        UpdateUserProfileError::AuthenticatedActorRequired | UpdateUserProfileError::Forbidden => {
            OperationError::Forbidden
        }
        UpdateUserProfileError::UserNotFound => OperationError::NotFound,
        UpdateUserProfileError::ConcurrencyConflict => OperationError::Conflict,
        error @ (UpdateUserProfileError::TemporarilyUnavailable { .. }
        | UpdateUserProfileError::BeginTransactionFailed(_)
        | UpdateUserProfileError::CommitTransactionFailed(_)) => {
            OperationError::Unavailable(Box::new(error))
        }
        error @ (UpdateUserProfileError::InvalidUserState { .. }
        | UpdateUserProfileError::InvalidPersistedState { .. }
        | UpdateUserProfileError::Internal { .. }) => OperationError::Internal(Box::new(error)),
    }
}

pub(super) fn address(error: AddressBookError) -> OperationError {
    match error {
        AddressBookError::AuthenticationRequired
        | AddressBookError::Forbidden
        | AddressBookError::Resolution(ResolutionError::Forbidden) => OperationError::Forbidden,
        AddressBookError::NotFound | AddressBookError::Resolution(ResolutionError::Missing) => {
            OperationError::NotFound
        }
        AddressBookError::Conflict | AddressBookError::Resolution(ResolutionError::Conflict) => {
            OperationError::Conflict
        }
        AddressBookError::InvalidInput
        | AddressBookError::Resolution(ResolutionError::InvalidInput) => {
            OperationError::InvalidInput
        }
        error @ (AddressBookError::Storage(_)
        | AddressBookError::Transaction(_)
        | AddressBookError::Resolution(
            ResolutionError::Storage(_) | ResolutionError::Transaction(_),
        )) => OperationError::Unavailable(Box::new(error)),
        error @ (AddressBookError::InvalidState(_)
        | AddressBookError::Resolution(
            ResolutionError::InvalidState(_) | ResolutionError::Provider(_),
        )) => OperationError::Internal(Box::new(error)),
    }
}
