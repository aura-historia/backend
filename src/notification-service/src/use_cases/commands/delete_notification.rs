use crate::ports::notification_deleter::{NotificationDeleteError, NotificationDeleter};
use application::operation_context::{OperationAuthorizationError, OperationContext, Principal};
use notification_core::notification_id::NotificationId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteNotificationCommand {
    pub notification_id: NotificationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteNotificationResult;

#[derive(Debug, thiserror::Error)]
pub enum DeleteNotificationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("notification not found")]
    NotFound,
    #[error("notification deletion failed")]
    DeleteFailed(#[from] NotificationDeleteError),
}

#[async_trait::async_trait]
pub trait DeleteNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteNotificationCommand,
    ) -> Result<DeleteNotificationResult, DeleteNotificationError>;
}

pub struct DeleteNotificationHandler<D> {
    deleter: D,
}

impl<D> DeleteNotificationHandler<D> {
    pub fn new(deleter: D) -> Self {
        Self { deleter }
    }
}

#[async_trait::async_trait]
impl<D> DeleteNotificationUseCase for DeleteNotificationHandler<D>
where
    D: NotificationDeleter,
{
    #[tracing::instrument(
        name = "delete_notification",
        skip_all,
        fields(
            notification_id = %command.notification_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteNotificationCommand,
    ) -> Result<DeleteNotificationResult, DeleteNotificationError> {
        let user_id = notification_owner(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let deleted = self
            .deleter
            .delete_one(user_id, command.notification_id)
            .await?;

        if !deleted {
            return Err(DeleteNotificationError::NotFound);
        }

        tracing::info!(
            event = "notification.deleted",
            actor_type = context.principal.kind(),
            actor_id = %user_id,
            notification_id = %command.notification_id,
            outcome = "success",
        );
        Ok(DeleteNotificationResult)
    }
}

fn notification_owner(context: &OperationContext) -> Result<UserId, DeleteNotificationError> {
    context
        .require()
        .any_user()
        .authorize::<DeleteNotificationError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(DeleteNotificationError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for DeleteNotificationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::box_error,
        operation_context::{CorrelationId, RequestId},
    };
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        result: bool,
        fail: bool,
        calls: Vec<(UserId, NotificationId)>,
    }

    #[derive(Clone, Default)]
    struct FakeDeleter(Arc<Mutex<State>>);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl NotificationDeleter for FakeDeleter {
        async fn delete_one(
            &self,
            user_id: UserId,
            notification_id: NotificationId,
        ) -> Result<bool, NotificationDeleteError> {
            let mut state = lock(&self.0);
            state.calls.push((user_id, notification_id));
            if state.fail {
                return Err(NotificationDeleteError::DeleteFailed {
                    source: box_error(std::io::Error::other("delete failed")),
                });
            }
            Ok(state.result)
        }

        async fn delete_all(&self, _: UserId) -> Result<u64, NotificationDeleteError> {
            unreachable!()
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[tokio::test]
    async fn should_delete_owned_notification_when_user_is_authenticated() {
        let user_id = UserId::new();
        let notification_id = NotificationId::new();
        let deleter = FakeDeleter::default();
        lock(&deleter.0).result = true;
        let handler = DeleteNotificationHandler::new(deleter.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                DeleteNotificationCommand { notification_id },
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(vec![(user_id, notification_id)], lock(&deleter.0).calls);
    }

    #[tokio::test]
    async fn should_return_not_found_when_owned_notification_is_missing_or_not_owned() {
        let deleter = FakeDeleter::default();
        let handler = DeleteNotificationHandler::new(deleter.clone());

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteNotificationCommand {
                    notification_id: NotificationId::new(),
                },
            )
            .await;

        assert!(matches!(result, Err(DeleteNotificationError::NotFound)));
        assert_eq!(1, lock(&deleter.0).calls.len());
    }

    #[tokio::test]
    async fn should_reject_anonymous_user_without_calling_deleter() {
        let deleter = FakeDeleter::default();
        let handler = DeleteNotificationHandler::new(deleter.clone());

        let result = handler
            .execute(
                &context(Principal::Anonymous),
                DeleteNotificationCommand {
                    notification_id: NotificationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(DeleteNotificationError::AuthenticatedActorRequired)
        ));
        assert!(lock(&deleter.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_deleter_failure() {
        let deleter = FakeDeleter::default();
        {
            let mut state = lock(&deleter.0);
            state.result = true;
            state.fail = true;
        }
        let handler = DeleteNotificationHandler::new(deleter);

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteNotificationCommand {
                    notification_id: NotificationId::new(),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(DeleteNotificationError::DeleteFailed(_))
        ));
    }
}
