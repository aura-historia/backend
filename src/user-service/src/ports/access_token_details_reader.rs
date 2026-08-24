use application::error::BoxError;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenDetails {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenDetailsReadError {
    #[error("temporary access token details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid access token details read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal access token details read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenDetailsReader: Send + Sync {
    async fn find_by_id(
        &self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<AccessTokenDetails>, AccessTokenDetailsReadError>;
}
