use super::OAuthClientReadError;
use credential_core::oauth_client_id::OAuthClientId;
use user_core::access_token::HashedRawOAuthClientSecret;

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientAuthentication {
    pub hashed_client_secret: HashedRawOAuthClientSecret,
}

#[async_trait::async_trait]
pub trait OAuthClientAuthenticationReader: Send + Sync {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientAuthentication>, OAuthClientReadError>;
}
