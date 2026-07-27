use crate::core::{tier::UserTier, user_aggregate::User};
use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierCommand {
    pub user_id: UserId,
    pub tier: UserTier,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierResult {
    pub user_id: UserId,
    pub tier: UserTier,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserTierError {
    #[error("user not found")]
    NotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait ChangeUserTierUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeUserTierCommand,
    ) -> Result<ChangeUserTierResult, ChangeUserTierError>;
}

impl From<&User> for ChangeUserTierResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            tier: user.account().tier,
        }
    }
}
