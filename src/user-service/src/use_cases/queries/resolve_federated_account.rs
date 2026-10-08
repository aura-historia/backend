use serde_email::Email;

use crate::ports::{
    CognitoIdentity, CognitoIssuer, FederatedAccountReadError, FederatedAccountReader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingEmailAction {
    LinkVerified,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveFederatedAccountCommand {
    pub email: Email,
    pub email_verified: bool,
    pub current_issuer: CognitoIssuer,
    pub existing_email_action: ExistingEmailAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederatedAccountDecision {
    AllowNewAccount,
    RejectExistingAccount,
    LinkExistingAccount {
        identity: CognitoIdentity,
        canonical_email: Email,
    },
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ResolveFederatedAccountError {
    #[error("federated account lookup is temporarily unavailable")]
    TemporarilyUnavailable,
    #[error("invalid persisted federated account state")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait ResolveFederatedAccountUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ResolveFederatedAccountCommand,
    ) -> Result<FederatedAccountDecision, ResolveFederatedAccountError>;
}

pub struct ResolveFederatedAccountHandler<R> {
    reader: R,
}

impl<R> ResolveFederatedAccountHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R: FederatedAccountReader> ResolveFederatedAccountUseCase
    for ResolveFederatedAccountHandler<R>
{
    async fn execute(
        &self,
        command: ResolveFederatedAccountCommand,
    ) -> Result<FederatedAccountDecision, ResolveFederatedAccountError> {
        let Some(account) = self
            .reader
            .find_by_email(&command.email)
            .await
            .map_err(map_read_error)?
        else {
            return Ok(FederatedAccountDecision::AllowNewAccount);
        };

        let persisted_email: &str = account.email.as_ref();
        let requested_email: &str = command.email.as_ref();
        if persisted_email != requested_email {
            return Err(ResolveFederatedAccountError::InvalidPersistedState);
        }

        match command.existing_email_action {
            ExistingEmailAction::Reject => Ok(FederatedAccountDecision::RejectExistingAccount),
            ExistingEmailAction::LinkVerified => {
                if !command.email_verified {
                    return Ok(FederatedAccountDecision::RejectExistingAccount);
                }

                let identity = account
                    .cognito_identity
                    .ok_or(ResolveFederatedAccountError::InvalidPersistedState)?;
                if identity.issuer != command.current_issuer {
                    return Err(ResolveFederatedAccountError::InvalidPersistedState);
                }

                Ok(FederatedAccountDecision::LinkExistingAccount {
                    identity,
                    canonical_email: account.email,
                })
            }
        }
    }
}

fn map_read_error(error: FederatedAccountReadError) -> ResolveFederatedAccountError {
    match error {
        FederatedAccountReadError::TemporarilyUnavailable { .. } => {
            ResolveFederatedAccountError::TemporarilyUnavailable
        }
        FederatedAccountReadError::InvalidPersistedState { .. } => {
            ResolveFederatedAccountError::InvalidPersistedState
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{CognitoSubject, FederatedAccount, FederatedAccountReadError};
    use application::error::static_error;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeReader {
        account: Option<FederatedAccount>,
        failure: bool,
        calls: Mutex<Vec<Email>>,
    }

    #[async_trait::async_trait]
    impl FederatedAccountReader for FakeReader {
        async fn find_by_email(
            &self,
            email: &Email,
        ) -> Result<Option<FederatedAccount>, FederatedAccountReadError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(email.clone());
            if self.failure {
                return Err(FederatedAccountReadError::TemporarilyUnavailable {
                    source: static_error("temporary test failure"),
                });
            }
            Ok(self.account.clone())
        }
    }

    fn email(value: &str) -> Email {
        value
            .try_into()
            .unwrap_or_else(|error| panic!("invalid test email: {error}"))
    }

    fn issuer(value: &str) -> CognitoIssuer {
        CognitoIssuer::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test issuer: {error}"))
    }

    fn account(email_value: &str, identity: Option<CognitoIdentity>) -> FederatedAccount {
        FederatedAccount {
            email: email(email_value),
            cognito_identity: identity,
        }
    }

    fn identity(issuer_value: &str, subject_value: &str) -> CognitoIdentity {
        CognitoIdentity {
            issuer: issuer(issuer_value),
            subject: CognitoSubject::try_from(subject_value)
                .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
        }
    }

    fn command(action: ExistingEmailAction, verified: bool) -> ResolveFederatedAccountCommand {
        ResolveFederatedAccountCommand {
            email: email("person@example.test"),
            email_verified: verified,
            current_issuer: issuer("https://cognito-idp.eu-central-1.amazonaws.com/pool"),
            existing_email_action: action,
        }
    }

    #[tokio::test]
    async fn no_postgres_email_match_allows_new_federation_without_verified_email() {
        let handler = ResolveFederatedAccountHandler::new(FakeReader::default());

        let result = handler
            .execute(command(ExistingEmailAction::LinkVerified, false))
            .await;

        assert_eq!(Ok(FederatedAccountDecision::AllowNewAccount), result);
    }

    #[tokio::test]
    async fn reject_action_never_links_a_colliding_email() {
        let handler = ResolveFederatedAccountHandler::new(FakeReader {
            account: Some(account("person@example.test", None)),
            ..FakeReader::default()
        });

        let result = handler
            .execute(command(ExistingEmailAction::Reject, true))
            .await;

        assert_eq!(Ok(FederatedAccountDecision::RejectExistingAccount), result);
    }

    #[tokio::test]
    async fn google_collision_requires_verified_source_and_same_pool_binding() {
        let account = account(
            "person@example.test",
            Some(identity(
                "https://cognito-idp.eu-central-1.amazonaws.com/pool",
                "canonical-sub",
            )),
        );
        let handler = ResolveFederatedAccountHandler::new(FakeReader {
            account: Some(account),
            ..FakeReader::default()
        });

        let unverified = handler
            .execute(command(ExistingEmailAction::LinkVerified, false))
            .await;
        let verified = handler
            .execute(command(ExistingEmailAction::LinkVerified, true))
            .await;

        assert_eq!(
            Ok(FederatedAccountDecision::RejectExistingAccount),
            unverified
        );
        assert_eq!(
            Ok(FederatedAccountDecision::LinkExistingAccount {
                identity: identity(
                    "https://cognito-idp.eu-central-1.amazonaws.com/pool",
                    "canonical-sub",
                ),
                canonical_email: email("person@example.test"),
            }),
            verified
        );
    }

    #[tokio::test]
    async fn missing_or_wrong_issuer_binding_fails_closed() {
        for identity_value in [
            None,
            Some(identity(
                "https://cognito-idp.eu-central-1.amazonaws.com/other-pool",
                "canonical-sub",
            )),
        ] {
            let handler = ResolveFederatedAccountHandler::new(FakeReader {
                account: Some(account("person@example.test", identity_value)),
                ..FakeReader::default()
            });

            assert_eq!(
                Err(ResolveFederatedAccountError::InvalidPersistedState),
                handler
                    .execute(command(ExistingEmailAction::LinkVerified, true))
                    .await
            );
        }
    }

    #[tokio::test]
    async fn database_read_failure_fails_closed() {
        let handler = ResolveFederatedAccountHandler::new(FakeReader {
            failure: true,
            ..FakeReader::default()
        });

        assert_eq!(
            Err(ResolveFederatedAccountError::TemporarilyUnavailable),
            handler
                .execute(command(ExistingEmailAction::Reject, false))
                .await
        );
    }
}
