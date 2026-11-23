//! Test doubles shared by the adapters' unit tests.

use google_cloud_auth::credentials::{
    AccessToken, AccessTokenCredentialsProvider, CacheableResource, CredentialsProvider,
};
use google_cloud_auth::errors::CredentialsError;

#[derive(Debug)]
pub(crate) enum TestCredentials {
    Issued,
    Stalled,
}

impl CredentialsProvider for TestCredentials {
    async fn headers(
        &self,
        _: http::Extensions,
    ) -> Result<CacheableResource<http::HeaderMap>, CredentialsError> {
        Err(CredentialsError::from_msg(
            false,
            "test credentials issue access tokens only",
        ))
    }

    async fn universe_domain(&self) -> Option<String> {
        None
    }
}

impl AccessTokenCredentialsProvider for TestCredentials {
    async fn access_token(&self) -> Result<AccessToken, CredentialsError> {
        match self {
            Self::Issued => Ok(AccessToken {
                token: "test-access-token".to_owned(),
            }),
            Self::Stalled => std::future::pending().await,
        }
    }
}
