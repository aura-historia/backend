use crate::core::{role::UserRole, user_aggregate::User};
use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleCommand {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleResult {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserRoleError {
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
pub trait ChangeUserRoleUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeUserRoleCommand,
    ) -> Result<ChangeUserRoleResult, ChangeUserRoleError>;
}

impl From<&User> for ChangeUserRoleResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            role: user.account().role,
        }
    }
}
