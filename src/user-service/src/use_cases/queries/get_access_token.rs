use common::operation_context::OperationContext;
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct GetAccessTokenRequest {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenView {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAccessTokenError {}

#[async_trait::async_trait]
pub trait GetAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAccessTokenRequest,
    ) -> Result<AccessTokenView, GetAccessTokenError>;
}
