use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserAdminError {
    #[error("user not found")]
    NotFound,
    #[error("admin role required")]
    AdminRoleRequired,
    #[error("temporary read failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait CheckUserAdminUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError>;
}
