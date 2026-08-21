use crate::ports::notification_seen_writer::{NotificationSeenWriteError, NotificationSeenWriter};
use application::operation_context::{OperationAuthorizationError, OperationContext, Principal};
use notification_core::notification_id::NotificationId;
use std::collections::HashSet;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationsSeenCommand {
    pub notification_ids: Vec<NotificationId>,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationsSeenResult {
    pub updated_count: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateNotificationsSeenError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("at least one notification ID is required")]
    EmptyNotificationIds,
    #[error("notification seen-state update failed")]
    UpdateFailed(#[from] NotificationSeenWriteError),
}

#[async_trait::async_trait]
pub trait UpdateNotificationsSeenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateNotificationsSeenCommand,
    ) -> Result<UpdateNotificationsSeenResult, UpdateNotificationsSeenError>;
}

pub struct UpdateNotificationsSeenHandler<W> {
    writer: W,
}

impl<W> UpdateNotificationsSeenHandler<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}

#[async_trait::async_trait]
impl<W> UpdateNotificationsSeenUseCase for UpdateNotificationsSeenHandler<W>
where
    W: NotificationSeenWriter,
{
    #[tracing::instrument(
        name = "update_notifications_seen",
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
        command: UpdateNotificationsSeenCommand,
    ) -> Result<UpdateNotificationsSeenResult, UpdateNotificationsSeenError> {
        let user_id = notification_owner(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));
        let notification_ids = unique_notification_ids(command.notification_ids);
        if notification_ids.is_empty() {
            return Err(UpdateNotificationsSeenError::EmptyNotificationIds);
        }

        let updated_count = self
            .writer
            .set_seen_many(user_id, &notification_ids, command.seen)
            .await?;

        tracing::info!(
            event = "notifications.seen_updated",
            actor_type = context.principal.kind(),
            actor_id = %user_id,
            notification_count = notification_ids.len(),
            updated_count,
            seen = command.seen,
            outcome = "success",
        );
        Ok(UpdateNotificationsSeenResult { updated_count })
    }
}

fn unique_notification_ids(notification_ids: Vec<NotificationId>) -> Vec<NotificationId> {
    let mut unique = HashSet::new();
    notification_ids
        .into_iter()
        .filter(|notification_id| unique.insert(*notification_id))
        .collect()
}

fn notification_owner(context: &OperationContext) -> Result<UserId, UpdateNotificationsSeenError> {
    context
        .require()
        .any_user()
        .authorize::<UpdateNotificationsSeenError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(UpdateNotificationsSeenError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for UpdateNotificationsSeenError {
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
        result: u64,
        fail: bool,
        calls: Vec<(UserId, Vec<NotificationId>, bool)>,
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
            user_id: UserId,
            notification_ids: &[NotificationId],
            seen: bool,
        ) -> Result<u64, NotificationSeenWriteError> {
            let mut state = lock(&self.0);
            state.calls.push((user_id, notification_ids.to_vec(), seen));
            if state.fail {
                return Err(NotificationSeenWriteError::UpdateFailed {
                    source: box_error(std::io::Error::other("write failed")),
                });
            }
            Ok(state.result)
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
    async fn should_update_unique_owned_notifications_when_user_is_authenticated() {
        let user_id = UserId::new();
        let first = NotificationId::new();
        let second = NotificationId::new();
        let writer = FakeWriter::default();
        lock(&writer.0).result = 2;
        let handler = UpdateNotificationsSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::User(user_id)),
                UpdateNotificationsSeenCommand {
                    notification_ids: vec![first, second, first],
                    seen: true,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(UpdateNotificationsSeenResult { updated_count: 2 })
        ));
        assert_eq!(
            vec![(user_id, vec![first, second], true)],
            lock(&writer.0).calls
        );
    }

    #[tokio::test]
    async fn should_reject_empty_batch_without_calling_writer() {
        let writer = FakeWriter::default();
        let handler = UpdateNotificationsSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                UpdateNotificationsSeenCommand {
                    notification_ids: Vec::new(),
                    seen: false,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationsSeenError::EmptyNotificationIds)
        ));
        assert!(lock(&writer.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_reject_service_principal_without_calling_writer() {
        let writer = FakeWriter::default();
        let handler = UpdateNotificationsSeenHandler::new(writer.clone());

        let result = handler
            .execute(
                &context(Principal::Service("worker".to_owned())),
                UpdateNotificationsSeenCommand {
                    notification_ids: vec![NotificationId::new()],
                    seen: true,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationsSeenError::Forbidden)
        ));
        assert!(lock(&writer.0).calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_seen_writer_failure() {
        let writer = FakeWriter::default();
        lock(&writer.0).fail = true;
        let handler = UpdateNotificationsSeenHandler::new(writer);

        let result = handler
            .execute(
                &context(Principal::User(UserId::new())),
                UpdateNotificationsSeenCommand {
                    notification_ids: vec![NotificationId::new()],
                    seen: true,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationsSeenError::UpdateFailed(_))
        ));
    }
}
