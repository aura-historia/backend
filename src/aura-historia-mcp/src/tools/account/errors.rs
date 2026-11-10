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
        GetOwnUserError::NotFound => OperationError::NotFound("Account not found: sign in again."),
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
        UpdateUserProfileError::UserNotFound => {
            OperationError::NotFound("Account not found: sign in again.")
        }
        UpdateUserProfileError::ConcurrencyConflict => {
            OperationError::conflict("Account changed concurrently: re-read the account and retry.")
        }
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
            OperationError::NotFound("Address not found: check the addressId.")
        }
        AddressBookError::Conflict | AddressBookError::Resolution(ResolutionError::Conflict) => {
            OperationError::conflict(
                "Stale expectedVersion: re-read the address or browsing destination and retry.",
            )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_tell_a_stale_version_to_reread_and_retry() {
        for error in [
            AddressBookError::Conflict,
            AddressBookError::Resolution(ResolutionError::Conflict),
        ] {
            assert_eq!(
                vec![
                    "CONFLICT",
                    "Stale expectedVersion: re-read the address or browsing destination and retry."
                ],
                address(error).tool_error_texts()
            );
        }
        let profile = update_profile(UpdateUserProfileError::ConcurrencyConflict);
        assert!(profile.tool_error_texts()[1].contains("re-read the account"));
    }

    #[test]
    fn should_name_the_address_or_account_that_was_not_found() {
        let address = address(AddressBookError::NotFound).tool_error_texts();
        assert_eq!(
            vec!["NOT_FOUND", "Address not found: check the addressId."],
            address
        );
        let account = get_profile(GetOwnUserError::NotFound).tool_error_texts();
        assert_eq!("NOT_FOUND", account[0]);
        assert!(account[1].starts_with("Account not found"), "{account:?}");
    }
}
