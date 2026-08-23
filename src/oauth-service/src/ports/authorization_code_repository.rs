use super::third_party_exchange_code_repository::OAuthCodeRepositoryError;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};

#[async_trait::async_trait]
pub trait AuthorizationCodeRepository: Send + Sync {
    async fn insert(&self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;
    async fn find_by_code(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError>;
    async fn delete(&self, code: &OAuthAuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;
}
