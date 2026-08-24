use super::OAuthCodeRepositoryError;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};

#[async_trait::async_trait]
pub trait AuthorizationCodeRepository: Send {
    async fn insert(&mut self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;

    async fn consume_by_code(
        &mut self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError>;
}

pub trait AuthorizationCodeRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuthorizationCodeRepository + 'tx;
}
