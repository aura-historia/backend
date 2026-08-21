use crate::ports::notification_seen_writer::{NotificationSeenWriteError, NotificationSeenWriter};
use application::operation_context::{OperationAuthorizationError, OperationContext, Principal};
use notification_core::notification_id::NotificationId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationSeenCommand {
    pub notification_id: NotificationId,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationSeenResult;

#[derive(Debug, thiserror::Error)]
pub enum UpdateNotificationSeenError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("notification not found")]
    NotFound,
    #[error("notification seen-state update failed")]
    UpdateFailed(#[from] NotificationSeenWriteError),
}

#[async_trait::async_trait]
pub trait UpdateNotificationSeenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateNotificationSeenCommand,
    ) -> Result<UpdateNotificationSeenResult, UpdateNotificationSeenError>;
}

pub struct UpdateNotificationSeenHandler<W> {
    writer: W,
}

impl<W> UpdateNotificationSeenHandler<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

#[async_trait::async_trait]
impl<W> UpdateNotificationSeenUseCase for UpdateNotificationSeenHandler<W>
where
    W: NotificationSeenWriter,
{
    #[tracing::instrument(
        name = "update_notification_seen",
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
        command: UpdateNotificationSeenCommand,
    ) -> Result<UpdateNotificationSeenResult, UpdateNotificationSeenError> {
        let user_id = notification_owner(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let updated = self
            .writer
            .set_seen(user_id, command.notification_id, command.seen)
            .await?;

        if !updated {
            return Err(UpdateNotificationSeenError::NotFound);
        }

        tracing::info!(
            event = "notification.seen_updated",
            actor_type = context.principal.kind(),
            actor_id = %user_id,
            notification_id = %command.notification_id,
            seen = command.seen,
            outcome = "success",
        );
        Ok(UpdateNotificationSeenResult)
    }
}

fn notification_owner(context: &OperationContext) -> Result<UserId, UpdateNotificationSeenError> {
    context
        .require()
        .any_user()
        .authorize::<UpdateNotificationSeenError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(UpdateNotificationSeenError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for UpdateNotificationSeenError {
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
        calls: Vec<(UserId, NotificationId, bool)>,
    }

    #[derive(Clone, Default)]
    struct FakeWriter(Arc<Mutex<State>>);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl NotificationSeenWriter for FakeWriter {
        async fn set_seen(
            &self,
            user_id: UserId,
            notification_id: NotificationId,
            seen: bool,
        ) -> Result<bool, NotificationSeenWriteError> {
            let mut state = lock(&self.0);
            state.calls.push((user_id, notification_id, seen));
            if state.fail {
                return Err(NotificationSeenWriteError::UpdateFailed {
                    source: box_error(std::io::Error::other("write failed")),
                });
            }
            Ok(state.result)
        }

        async fn set_seen_many(
            &self,
            _: UserId,
            _: &[NotificationId],
            _: bool,
        ) -> Result<u64, NotificationSeenWriteError> {
            unreachable!()
        }

        async fn set_seen_all(
            &self,
            _: UserId,
            _: bool,
        ) -> Result<u64, NotificationSeenWriteError> {
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
    async fn should_update_owned_notification_when_user_is_authenticated() {
        let user_id = UserId::new();
        let notification_id = NotificationId::new();
        let writer = FakeWriter::default();
        lock(&writer.0).result = true;
        let handler = UpdateNotificationSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                UpdateNotificationSeenCommand {
                    notification_id,
                    seen: true,
                },
            )
            .await;

        assert!(result.is_ok());
        assert_eq!(
            vec![(user_id, notification_id, true)],
            lock(&writer.0).calls
        );
    }

    #[tokio::test]
    async fn should_return_not_found_when_owned_notification_is_missing_or_not_owned() {
        let user_id = UserId::new();
        let writer = FakeWriter::default();
        let handler = UpdateNotificationSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                UpdateNotificationSeenCommand {
                    notification_id: NotificationId::new(),
                    seen: true,
                },
            )
            .await;

        assert!(matches!(result, Err(UpdateNotificationSeenError::NotFound)));
        assert_eq!(1, lock(&writer.0).calls.len());
    }

    #[tokio::test]
    async fn should_reject_anonymous_user_without_calling_writer() {
        let writer = FakeWriter::default();
        let handler = UpdateNotificationSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::Anonymous),
                UpdateNotificationSeenCommand {
                    notification_id: NotificationId::new(),
                    seen: true,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationSeenError::AuthenticatedActorRequired)
        ));
        assert!(lock(&writer.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_seen_writer_failure() {
        let user_id = UserId::new();
        let writer = FakeWriter::default();
        {
            let mut state = lock(&writer.0);
            state.result = true;
            state.fail = true;
        }
        let handler = UpdateNotificationSeenHandler::new(writer);

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                UpdateNotificationSeenCommand {
                    notification_id: NotificationId::new(),
                    seen: true,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationSeenError::UpdateFailed(_))
        ));
    }
}
