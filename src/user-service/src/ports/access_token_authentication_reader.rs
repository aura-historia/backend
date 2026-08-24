use application::error::BoxError;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenOrigin, HashedRawAccessToken, Scope};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenAuthentication {
    pub access_token_id: AccessTokenId,
    pub user_id: UserId,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenAuthenticationReadError {
    #[error("temporary access token authentication read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid access token authentication read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal access token authentication read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenAuthenticationReader: Send + Sync {
    async fn find_authentication_by_hashed_token(
        &self,
        hashed_token: &HashedRawAccessToken,
    ) -> Result<Option<AccessTokenAuthentication>, AccessTokenAuthenticationReadError>;
}
