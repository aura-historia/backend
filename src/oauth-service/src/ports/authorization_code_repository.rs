use super::OAuthCodeRepositoryError;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};
use oauth_core::client_identifier::OAuthClientIdentifier;
use user_core::user_id::UserId;

#[async_trait::async_trait]
pub trait AuthorizationCodeRepository: Send {
    async fn insert(&mut self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;

    async fn consume_by_code(
        &mut self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError>;

    /// Deletes the user's outstanding codes for the client, so no approval survives a
    /// disconnect.
    async fn delete_for_user_and_client(
        &mut self,
        user_id: UserId,
        client_id: &OAuthClientIdentifier,
    ) -> Result<u64, OAuthCodeRepositoryError>;
}

pub trait AuthorizationCodeRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuthorizationCodeRepository + 'tx;
}
