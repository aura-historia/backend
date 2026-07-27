use crate::core::access_token::AccessTokenId;
use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteAccessTokenError {
    #[error("access token not found")]
    NotFound,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary access token store failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait DeleteAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError>;
}
