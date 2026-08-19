use crate::ports::notification_seen_writer::{NotificationSeenWriteError, NotificationSeenWriter};
use common::{
    operation_context::{OperationAuthorizationError, OperationContext, Principal},
    user_id::UserId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAllNotificationsSeenCommand {
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAllNotificationsSeenResult {
    pub updated_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateAllNotificationsSeenError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("notification seen-state update failed")]
    UpdateFailed(#[from] NotificationSeenWriteError),
}

#[async_trait::async_trait]
pub trait UpdateAllNotificationsSeenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAllNotificationsSeenCommand,
    ) -> Result<UpdateAllNotificationsSeenResult, UpdateAllNotificationsSeenError>;
}

pub struct UpdateAllNotificationsSeenHandler<W> {
    writer: W,
}

impl<W> UpdateAllNotificationsSeenHandler<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

#[async_trait::async_trait]
impl<W> UpdateAllNotificationsSeenUseCase for UpdateAllNotificationsSeenHandler<W>
where
    W: NotificationSeenWriter,
{
    #[tracing::instrument(
        name = "update_all_notifications_seen",
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
        command: UpdateAllNotificationsSeenCommand,
    ) -> Result<UpdateAllNotificationsSeenResult, UpdateAllNotificationsSeenError> {
        let user_id = notification_owner(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let updated_count = self.writer.set_seen_all(user_id, command.seen).await?;

        tracing::info!(
            event = "notifications.all_seen_updated",
            actor_type = context.principal.kind(),
            actor_id = %user_id,
            updated_count,
            seen = command.seen,
            outcome = "success",
        );
        Ok(UpdateAllNotificationsSeenResult { updated_count })
    }
}

fn notification_owner(
    context: &OperationContext,
) -> Result<UserId, UpdateAllNotificationsSeenError> {
    context
        .require()
        .any_user()
        .authorize::<UpdateAllNotificationsSeenError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(UpdateAllNotificationsSeenError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for UpdateAllNotificationsSeenError {
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
    use common::{
        error::boxed::box_error,
        notification_id::NotificationId,
        operation_context::{CorrelationId, RequestId},
    };
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        result: u64,
        fail: bool,
        calls: Vec<(UserId, bool)>,
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
            _: UserId,
            _: NotificationId,
            _: bool,
        ) -> Result<bool, NotificationSeenWriteError> {
            unreachable!()
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
            user_id: UserId,
            seen: bool,
        ) -> Result<u64, NotificationSeenWriteError> {
            let mut state = lock(&self.0);
            state.calls.push((user_id, seen));
            if state.fail {
                return Err(NotificationSeenWriteError::UpdateFailed {
                    source: box_error(std::io::Error::other("write failed")),
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
    async fn should_update_all_notifications_for_context_owner() {
        let user_id = UserId::new();
        let writer = FakeWriter::default();
        lock(&writer.0).result = 3;
        let handler = UpdateAllNotificationsSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                UpdateAllNotificationsSeenCommand { seen: true },
            )
            .await;

        assert!(matches!(
            result,
            Ok(UpdateAllNotificationsSeenResult { updated_count: 3 })
        ));
        assert_eq!(vec![(user_id, true)], lock(&writer.0).calls);
    }

    #[tokio::test]
    async fn should_reject_anonymous_user_without_calling_writer() {
        let writer = FakeWriter::default();
        let handler = UpdateAllNotificationsSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::Anonymous),
                UpdateAllNotificationsSeenCommand { seen: false },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateAllNotificationsSeenError::AuthenticatedActorRequired)
        ));
        assert!(lock(&writer.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_seen_writer_failure() {
        let writer = FakeWriter::default();
        lock(&writer.0).fail = true;
        let handler = UpdateAllNotificationsSeenHandler::new(writer);

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                UpdateAllNotificationsSeenCommand { seen: true },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateAllNotificationsSeenError::UpdateFailed(_))
        ));
    }
}
