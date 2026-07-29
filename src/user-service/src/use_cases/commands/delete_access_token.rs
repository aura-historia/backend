use common::operation_context::OperationContext;
use common::user_id::UserId;
use user_core::access_token::AccessTokenId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteAccessTokenError {}

#[async_trait::async_trait]
pub trait DeleteAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteAccessTokenCommand,
    ) -> Result<DeleteAccessTokenResult, DeleteAccessTokenError>;
}
