use crate::ports::{
    notification_channel_sender::{
        NotificationChannelSendError, NotificationDeliveryDispatchError,
        NotificationDeliveryDispatcher,
    },
    notification_delivery_repository::{
        ClaimNotificationDeliveryOutcome, NotificationDeliveryError, NotificationDeliveryRepository,
    },
};
use notification_core::{
    notification_delivery::NotificationDeliveryChannel,
    notification_delivery_id::NotificationDeliveryId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const DELIVERY_LEASE_DURATION: Duration = Duration::minutes(5);
const UNREGISTERED_CHANNEL_ERROR_CODE: &str = "NOTIFICATION_CHANNEL_UNREGISTERED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverNotificationCommand {
    pub notification_delivery_id: NotificationDeliveryId,
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
    RetryableSend(#[source] NotificationChannelSendError),
    #[error("notification channel sender is not registered for {channel:?}")]
    UnregisteredChannel {
        channel: NotificationDeliveryChannel,
    },

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

pub struct DeliverNotificationHandler<R> {
    deliveries: R,
    dispatcher: NotificationDeliveryDispatcher,
}

impl<R> DeliverNotificationHandler<R> {
    pub fn new(deliveries: R, dispatcher: NotificationDeliveryDispatcher) -> Self {
        Self {
            deliveries,
            dispatcher,
        }
    }
}

#[async_trait::async_trait]
impl<R> DeliverNotificationUseCase for DeliverNotificationHandler<R>
where
    R: NotificationDeliveryRepository,
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

        let Some(source) = *source else {
            let completed = self
                .deliveries
                .mark_permanent_failure(
                    claimed.notification_delivery_id,
                    claimed.lease_token,
                    "NOTIFICATION_SOURCE_MISSING",
                    OffsetDateTime::now_utc(),
                )
                .await?;
            return if completed {
                Ok(DeliverNotificationResult::SourceMissing)
            } else {
                Err(DeliverNotificationError::LeaseLost)
            };
        };

        match self.dispatcher.dispatch(&source).await {
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
            Err(NotificationDeliveryDispatchError::UnregisteredChannel { channel }) => {
                let completed = self
                    .deliveries
                    .mark_permanent_failure(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        UNREGISTERED_CHANNEL_ERROR_CODE,
                        OffsetDateTime::now_utc(),
                    )
                    .await?;
                if completed {
                    Err(DeliverNotificationError::UnregisteredChannel { channel })
                } else {
                    Err(DeliverNotificationError::LeaseLost)
                }
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Retryable { .. },
            )) => {
                let completed = self
                    .deliveries
                    .mark_retryable_failure(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        error.code(),
                        OffsetDateTime::now_utc(),
                    )
                    .await?;
                if completed {
                    Err(DeliverNotificationError::RetryableSend(error))
                } else {
                    Err(DeliverNotificationError::LeaseLost)
                }
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Permanent { .. },
            )) => {
                let completed = self
                    .deliveries
                    .mark_permanent_failure(
                        claimed.notification_delivery_id,
                        claimed.lease_token,
                        error.code(),
                        OffsetDateTime::now_utc(),
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
        notification_channel_sender::{
            NotificationChannelSender, NotificationDeliveryDispatcher,
            NotificationDeliveryDispatcherRegistrationError, SentNotificationDelivery,
        },
        notification_delivery_repository::{
            ClaimedNotificationDelivery, NotificationDeliverySource,
        },
    };
    use common::{
        currency::domain::Currency, error::boxed::box_error, language::domain::Language,
        notification_id::NotificationId, partner_shop_application_id::PartnerShopApplicationId,
        shop_name::ShopName, user_id::UserId,
    };
    use notification_core::{
        notification::{
            NotificationContent, PartnerApplicationDecision, PartnerApplicationNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct DeliveryState {
        claimed_delivery_ids: Mutex<Vec<NotificationDeliveryId>>,
        delivered_message_ids: Mutex<Vec<String>>,
        permanent_failure_codes: Mutex<Vec<String>>,
    }

    struct FakeDeliveryRepository {
        claimed: ClaimedNotificationDelivery,
        source: NotificationDeliverySource,
        state: Arc<DeliveryState>,
    }

    impl FakeDeliveryRepository {
        fn new(
            notification_delivery_id: NotificationDeliveryId,
            source: NotificationDeliverySource,
            state: Arc<DeliveryState>,
        ) -> Self {
            Self {
                claimed: ClaimedNotificationDelivery {
                    notification_delivery_id,
                    notification_id: source.notification_id,
                    lease_token: Uuid::now_v7(),
                    lease_expires_at: OffsetDateTime::now_utc() + DELIVERY_LEASE_DURATION,
                    attempt_count: 2,
                },
                source,
                state,
            }
        }
    }

    #[async_trait::async_trait]
    impl NotificationDeliveryRepository for FakeDeliveryRepository {
        async fn claim_and_load_source(
            &self,
            notification_delivery_id: NotificationDeliveryId,
            _: OffsetDateTime,
            _: OffsetDateTime,
            _: Uuid,
        ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
            self.state
                .claimed_delivery_ids
                .lock()
                .map_err(|_| repository_error())?
                .push(notification_delivery_id);
            Ok(ClaimNotificationDeliveryOutcome::Claimed {
                delivery: self.claimed.clone(),
                source: Box::new(Some(self.source.clone())),
            })
        }

        async fn mark_delivered(
            &self,
            _: NotificationDeliveryId,
            _: Uuid,
            provider_message_id: &str,
            _: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .delivered_message_ids
                .lock()
                .map_err(|_| repository_error())?
                .push(provider_message_id.to_owned());
            Ok(true)
        }

        async fn mark_retryable_failure(
            &self,
            _: NotificationDeliveryId,
            _: Uuid,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            Ok(true)
        }

        async fn mark_permanent_failure(
            &self,
            _: NotificationDeliveryId,
            _: Uuid,
            error_code: &str,
            _: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .permanent_failure_codes
                .lock()
                .map_err(|_| repository_error())?
                .push(error_code.to_owned());
            Ok(true)
        }
    }

    #[derive(Default)]
    struct RecordingEmailSender {
        sent_sources: Mutex<Vec<NotificationDeliverySource>>,
    }

    #[async_trait::async_trait]
    impl NotificationChannelSender for RecordingEmailSender {
        fn channel(&self) -> NotificationDeliveryChannel {
            NotificationDeliveryChannel::Email
        }

        async fn send(
            &self,
            source: &NotificationDeliverySource,
        ) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
            self.sent_sources
                .lock()
                .map_err(|_| NotificationChannelSendError::Permanent {
                    code: "TEST_SENDER_LOCK_FAILED",
                    source: box_error(std::io::Error::other("test sender lock poisoned")),
                })?
                .push(source.clone());
            Ok(SentNotificationDelivery {
                provider_message_id: "provider-message-1".to_owned(),
            })
        }
    }

    fn source(notification_delivery_id: NotificationDeliveryId) -> NotificationDeliverySource {
        NotificationDeliverySource {
            notification_delivery_id,
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::PartnerApplication {
                partner_shop_application_id: PartnerShopApplicationId::new(),
                snapshot: PartnerApplicationNotificationSnapshot {
                    shop_name: ShopName::from("Test Shop"),
                    image: None,
                },
                decision: PartnerApplicationDecision::Approved,
            },
            language: Language::En,
            currency: Currency::Eur,
        }
    }

    fn repository_error() -> NotificationDeliveryError {
        NotificationDeliveryError::OperationFailed {
            source: box_error(std::io::Error::other("test repository lock poisoned")),
        }
    }

    #[tokio::test]
    async fn should_claim_send_and_finalize_through_registered_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let source = source(notification_delivery_id);
        let state = Arc::new(DeliveryState::default());
        let sender = Arc::new(RecordingEmailSender::default());
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(notification_delivery_id, source.clone(), state.clone()),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await?;

        assert_eq!(
            DeliverNotificationResult::Delivered { attempt_count: 2 },
            result
        );
        assert_eq!(
            vec![notification_delivery_id],
            state
                .claimed_delivery_ids
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        assert_eq!(
            vec![source],
            sender
                .sent_sources
                .lock()
                .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
                .clone()
        );
        assert_eq!(
            vec!["provider-message-1".to_owned()],
            state
                .delivered_message_ids
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        Ok(())
    }

    #[test]
    fn should_reject_duplicate_channel_registration() {
        let sender = Arc::new(RecordingEmailSender::default());

        let result = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>,
            sender as Arc<dyn NotificationChannelSender>,
        ]);

        assert!(matches!(
            result,
            Err(
                NotificationDeliveryDispatcherRegistrationError::DuplicateChannelRegistration {
                    channel: NotificationDeliveryChannel::Email,
                }
            )
        ));
    }

    #[tokio::test]
    async fn should_finalize_and_report_unregistered_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            NotificationDeliveryDispatcher::new(Vec::new())?,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await;

        assert!(matches!(
            result,
            Err(DeliverNotificationError::UnregisteredChannel {
                channel: NotificationDeliveryChannel::Email,
            })
        ));
        assert_eq!(
            vec![UNREGISTERED_CHANNEL_ERROR_CODE.to_owned()],
            state
                .permanent_failure_codes
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        Ok(())
    }
}
