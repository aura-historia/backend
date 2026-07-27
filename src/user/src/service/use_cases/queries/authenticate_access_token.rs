use crate::core::access_token::{HashedRawAccessToken, Scope};
use common::operation_context::OperationContext;
use common::user_id::UserId;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenRequest {
    pub hashed_token: HashedRawAccessToken,
    pub required_scopes: HashSet<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticateAccessTokenError {}

#[async_trait::async_trait]
pub trait AuthenticateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError>;
}
