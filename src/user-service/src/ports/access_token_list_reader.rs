use crate::ports::access_token_details_reader::AccessTokenDetails;
use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenListReadError {
    #[error("temporary access token list read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid access token list read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal access token list read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AccessTokenListReader: Send + Sync {
    async fn list_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AccessTokenDetails>, AccessTokenListReadError>;
}
