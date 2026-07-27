use crate::core::user_aggregate::{NewUser, User, UserAccount, UserPreferences, UserProfile};
use common::operation_context::OperationContext;
use common::user_id::UserId;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserCommand {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserResult {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("user already exists")]
    AlreadyExists,
    #[error("user email already exists")]
    EmailConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait CreateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateUserCommand,
    ) -> Result<CreateUserResult, CreateUserError>;
}

impl TryFrom<CreateUserCommand> for User {
    type Error = crate::core::user_aggregate::RehydrateUserError;

    fn try_from(command: CreateUserCommand) -> Result<Self, Self::Error> {
        User::create(NewUser {
            id: command.user_id,
            email: command.email,
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount::default(),
        })
    }
}

impl From<&User> for CreateUserResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            email: user.email().clone(),
        }
    }
}
