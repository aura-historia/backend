use crate::core::access_token::{
    AccessTokenId, AccessTokenName, AccessTokenOrigin, RawAccessToken, Scope,
};
use common::operation_context::OperationContext;
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenCommand {
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub expires: Option<OffsetDateTime>,
    pub origin: AccessTokenOrigin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateAccessTokenResult {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub raw_access_token: RawAccessToken,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAccessTokenError {}

#[async_trait::async_trait]
pub trait CreateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateAccessTokenCommand,
    ) -> Result<CreateAccessTokenResult, CreateAccessTokenError>;
}
