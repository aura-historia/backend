use crate::ports::notification_delivery_repository::{
    ClaimNotificationDeliveryOutcome, NotificationDeliveryError, NotificationDeliveryRepository,
};
use crate::ports::notification_delivery_sender::{
    NotificationDeliverySendError, NotificationDeliverySender,
};
use common::notification_id::NotificationId;
use notification_core::notification_delivery_id::NotificationDeliveryId;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const DELIVERY_LEASE_DURATION: Duration = Duration::minutes(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverNotificationCommand {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverNotificationResult {
    Delivered { attempt_count: u32 },
    DeliveryMissing,
    AlreadyDelivered,
    AlreadyClaimed,
    SourceMissing,
    PermanentlyFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum DeliverNotificationError {
    #[error("notification delivery repository operation failed")]
    Repository(#[from] NotificationDeliveryError),
    #[error("notification delivery send failed temporarily")]
    RetryableSend(#[source] NotificationDeliverySendError),
    #[error("notification delivery does not belong to the supplied notification")]
    NotificationMismatch,
    #[error("notification delivery lease was lost before finalization")]
    LeaseLost,
}

#[async_trait::async_trait]
pub trait DeliverNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: DeliverNotificationCommand,
    ) -> Result<DeliverNotificationResult, DeliverNotificationError>;
}

pub struct DeliverNotificationHandler<R, S> {
    deliveries: R,
    sender: S,
}

impl<R, S> DeliverNotificationHandler<R, S> {
    pub fn new(deliveries: R, sender: S) -> Self {
        Self { deliveries, sender }
    }
}

#[async_trait::async_trait]
impl<R, S> DeliverNotificationUseCase for DeliverNotificationHandler<R, S>
where
    R: NotificationDeliveryRepository,
    S: NotificationDeliverySender,
{
    #[tracing::instrument(
        name = "deliver_notification",
        skip(self),
        fields(notification_delivery_id = %command.notification_delivery_id)
    )]
    async fn execute(
        &self,
        command: DeliverNotificationCommand,
    ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
        let now = OffsetDateTime::now_utc();
        let lease_token = Uuid::now_v7();
        let (claimed, source) = match self
            .deliveries
            .claim_and_load_source(
                command.notification_delivery_id,
                now,
                now + DELIVERY_LEASE_DURATION,
                lease_token,
            )
            .await?
        {
            ClaimNotificationDeliveryOutcome::Missing => {
                return Ok(DeliverNotificationResult::DeliveryMissing);
            }
            ClaimNotificationDeliveryOutcome::Delivered => {
                return Ok(DeliverNotificationResult::AlreadyDelivered);
            }
            ClaimNotificationDeliveryOutcome::PermanentlyFailed => {
                return Ok(DeliverNotificationResult::PermanentlyFailed);
            }
            ClaimNotificationDeliveryOutcome::AlreadyClaimed => {
                return Ok(DeliverNotificationResult::AlreadyClaimed);
            }
            ClaimNotificationDeliveryOutcome::Claimed { delivery, source } => (delivery, source),
        };

        if claimed.notification_id != command.notification_id {
            return Err(DeliverNotificationError::NotificationMismatch);
        }

        let Some(source) = *source else {
            let completed = self
                .deliveries
                .mark_permanent_failure(
                    claimed.notification_delivery_id,
                    claimed.lease_token,
                    "NOTIFICATION_SOURCE_MISSING",
                )
                .await?;
            return if completed {
                Ok(DeliverNotificationResult::SourceMissing)
            } else {
                Err(DeliverNotificationError::LeaseLost)
            };
        };

        match self.sender.send(&source).await {
            Ok(sent) => {
                let completed = self
                    .deliveries
                    .mark_delivered(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        &sent.provider_message_id,
                        OffsetDateTime::now_utc(),
                    )
                    .await?;
                if completed {
                    Ok(DeliverNotificationResult::Delivered {
                        attempt_count: claimed.attempt_count,
                    })
                } else {
                    Err(DeliverNotificationError::LeaseLost)
                }
            }
            Err(error @ NotificationDeliverySendError::Retryable { .. }) => {
                let completed = self
                    .deliveries
                    .mark_retryable_failure(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        error.code(),
                    )
                    .await?;
                if completed {
                    Err(DeliverNotificationError::RetryableSend(error))
                } else {
                    Err(DeliverNotificationError::LeaseLost)
                }
            }
            Err(error @ NotificationDeliverySendError::Permanent { .. }) => {
                let completed = self
                    .deliveries
                    .mark_permanent_failure(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        error.code(),
                    )
                    .await?;
                if completed {
                    Ok(DeliverNotificationResult::PermanentlyFailed)
                } else {
                    Err(DeliverNotificationError::LeaseLost)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        notification_delivery_repository::MockNotificationDeliveryRepository,
        notification_delivery_sender::MockNotificationDeliverySender,
    };

    #[tokio::test]
    async fn should_skip_delivery_when_another_worker_holds_the_lease()
    -> Result<(), DeliverNotificationError> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let notification_id = NotificationId::new();
        let mut deliveries = MockNotificationDeliveryRepository::new();
        deliveries
            .expect_claim_and_load_source()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(ClaimNotificationDeliveryOutcome::AlreadyClaimed) })
            });
        let sender = MockNotificationDeliverySender::new();
        let handler = DeliverNotificationHandler::new(deliveries, sender);

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
                notification_id,
            })
            .await?;

        assert_eq!(DeliverNotificationResult::AlreadyClaimed, result);
        Ok(())
    }
}
