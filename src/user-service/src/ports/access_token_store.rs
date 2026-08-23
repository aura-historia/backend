#![allow(dead_code)]

use application::error::BoxError;
use user_core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenStoreError {
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token store failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenStore: Send + Sync {
    async fn find_by_id(
        &self,
        user_id: &UserId,
        access_token_id: &AccessTokenId,
    ) -> Result<Option<AccessToken>, AccessTokenStoreError>;

    async fn find_by_hashed_token(
        &self,
        hashed_token: &HashedRawAccessToken,
    ) -> Result<Option<AccessToken>, AccessTokenStoreError>;

    async fn list_for_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<AccessToken>, AccessTokenStoreError>;

    async fn insert(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError>;

    async fn replace(&self, access_token: AccessToken) -> Result<(), AccessTokenStoreError>;

    async fn delete(
        &self,
        user_id: &UserId,
        access_token_id: &AccessTokenId,
    ) -> Result<(), AccessTokenStoreError>;
}
