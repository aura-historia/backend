use common::operation_context::OperationContext;
use common::user_id::UserId;
use user_core::{tier::UserTier, user::User};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierCommand {
    pub user_id: UserId,
    pub tier: UserTier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierResult {
    pub user_id: UserId,
    pub tier: UserTier,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserTierError {}

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
