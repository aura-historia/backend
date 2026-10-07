use application::error::BoxError;
use serde_email::Email;

use super::cognito_identity::CognitoIdentity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedAccount {
    pub email: Email,
    pub cognito_identity: Option<CognitoIdentity>,
}

#[derive(Debug, thiserror::Error)]
pub enum FederatedAccountReadError {
    #[error("temporary federated account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted federated account state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// Reads the PostgreSQL account that owns an exact email and its canonical Cognito binding.
#[async_trait::async_trait]
pub trait FederatedAccountReader: Send + Sync {
    async fn find_by_email(
        &self,
        email: &Email,
    ) -> Result<Option<FederatedAccount>, FederatedAccountReadError>;
}
