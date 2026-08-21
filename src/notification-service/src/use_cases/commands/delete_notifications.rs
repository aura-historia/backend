use crate::ports::notification_deleter::{NotificationDeleteError, NotificationDeleter};
use application::operation_context::{OperationAuthorizationError, OperationContext, Principal};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteNotificationsCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteNotificationsResult {
    pub deleted_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteNotificationsError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("notification deletion failed")]
    DeleteFailed(#[from] NotificationDeleteError),
}

#[async_trait::async_trait]
pub trait DeleteNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteNotificationsCommand,
    ) -> Result<DeleteNotificationsResult, DeleteNotificationsError>;
}

pub struct DeleteNotificationsHandler<D> {
    deleter: D,
}

impl<D> DeleteNotificationsHandler<D> {
    pub fn new(deleter: D) -> Self {
        Self { deleter }
    }
}

#[async_trait::async_trait]
impl<D> DeleteNotificationsUseCase for DeleteNotificationsHandler<D>
where
    D: NotificationDeleter,
{
    #[tracing::instrument(
        name = "delete_notifications",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        _: DeleteNotificationsCommand,
    ) -> Result<DeleteNotificationsResult, DeleteNotificationsError> {
        let user_id = notification_owner(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let deleted_count = self.deleter.delete_all(user_id).await?;

        tracing::info!(
            event = "notifications.deleted",
            actor_type = context.principal.kind(),
            actor_id = %user_id,
            deleted_count,
            outcome = "success",
        );
        Ok(DeleteNotificationsResult { deleted_count })
    }
}

fn notification_owner(context: &OperationContext) -> Result<UserId, DeleteNotificationsError> {
    context
        .require()
        .any_user()
        .authorize::<DeleteNotificationsError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(DeleteNotificationsError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for DeleteNotificationsError {
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
    use notification_core::notification_id::NotificationId;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        result: u64,
        fail: bool,
        calls: Vec<UserId>,
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
            _: UserId,
            _: NotificationId,
        ) -> Result<bool, NotificationDeleteError> {
            unreachable!()
        }

        async fn delete_all(&self, user_id: UserId) -> Result<u64, NotificationDeleteError> {
            let mut state = lock(&self.0);
            state.calls.push(user_id);
            if state.fail {
                return Err(NotificationDeleteError::DeleteFailed {
                    source: box_error(std::io::Error::other("delete failed")),
                });
            }
            Ok(state.result)
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
    async fn should_delete_all_notifications_for_context_owner() {
        let user_id = UserId::new();
        let deleter = FakeDeleter::default();
        lock(&deleter.0).result = 4;
        let handler = DeleteNotificationsHandler::new(deleter.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                DeleteNotificationsCommand,
            )
            .await;

        assert!(matches!(
            result,
            Ok(DeleteNotificationsResult { deleted_count: 4 })
        ));
        assert_eq!(vec![user_id], lock(&deleter.0).calls);
    }

    #[tokio::test]
    async fn should_allow_idempotent_delete_when_no_notifications_exist() {
        let user_id = UserId::new();
        let deleter = FakeDeleter::default();
        let handler = DeleteNotificationsHandler::new(deleter.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                DeleteNotificationsCommand,
            )
            .await;

        assert!(matches!(
            result,
            Ok(DeleteNotificationsResult { deleted_count: 0 })
        ));
        assert_eq!(vec![user_id], lock(&deleter.0).calls);
    }

    #[tokio::test]
    async fn should_reject_system_principal_without_calling_deleter() {
        let deleter = FakeDeleter::default();
        let handler = DeleteNotificationsHandler::new(deleter.clone());

        let result = handler
            .execute(&context(Principal::System), DeleteNotificationsCommand)
            .await;

        assert!(matches!(result, Err(DeleteNotificationsError::Forbidden)));
        assert!(lock(&deleter.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_deleter_failure() {
        let deleter = FakeDeleter::default();
        lock(&deleter.0).fail = true;
        let handler = DeleteNotificationsHandler::new(deleter);

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                DeleteNotificationsCommand,
            )
            .await;

        assert!(matches!(
            result,
            Err(DeleteNotificationsError::DeleteFailed(_))
        ));
    }
}
