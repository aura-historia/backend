use crate::ports::{
    notification_channel_sender::{
        NotificationChannelSendError, NotificationDeliveryDispatchError,
        NotificationDeliveryDispatcher,
    },
    notification_delivery_repository::{
        ClaimNotificationDeliveryOutcome, ClaimedNotificationDelivery, NotificationDeliveryError,
        NotificationDeliveryRepository,
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
const FINALIZATION_INITIAL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
const FINALIZATION_MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

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

enum DeliveryCompletion {
    Delivered {
        provider_message_id: String,
        completed_at: OffsetDateTime,
    },
    RetryableFailure {
        error_code: &'static str,
        completed_at: OffsetDateTime,
    },
    PermanentFailure {
        error_code: &'static str,
        completed_at: OffsetDateTime,
    },
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

impl<R> DeliverNotificationHandler<R>
where
    R: NotificationDeliveryRepository,
{
    async fn finalize(
        &self,
        claimed: &ClaimedNotificationDelivery,
        completion: &DeliveryCompletion,
    ) -> Result<(), DeliverNotificationError> {
        let mut retry_delay = FINALIZATION_INITIAL_RETRY_DELAY;
        let mut retry_attempt = 1_u32;

        loop {
            let result = match completion {
                DeliveryCompletion::Delivered {
                    provider_message_id,
                    completed_at,
                } => {
                    self.deliveries
                        .mark_delivered(
                            claimed.notification_delivery_id,
                            claimed.lease_token,
                            provider_message_id,
                            *completed_at,
                        )
                        .await
                }
                DeliveryCompletion::RetryableFailure {
                    error_code,
                    completed_at,
                } => {
                    self.deliveries
                        .mark_retryable_failure(
                            claimed.notification_delivery_id,
                            claimed.lease_token,
                            error_code,
                            *completed_at,
                        )
                        .await
                }
                DeliveryCompletion::PermanentFailure {
                    error_code,
                    completed_at,
                } => {
                    self.deliveries
                        .mark_permanent_failure(
                            claimed.notification_delivery_id,
                            claimed.lease_token,
                            error_code,
                            *completed_at,
                        )
                        .await
                }
            };

            match result {
                Ok(true) => return Ok(()),
                Ok(false) => return Err(DeliverNotificationError::LeaseLost),
                Err(NotificationDeliveryError::OperationFailed { .. }) => {
                    tracing::warn!(
                        notification_delivery_id = %claimed.notification_delivery_id,
                        retry_attempt,
                        "notification delivery finalization failed; retrying"
                    );
                    if !retry_delay.is_zero() {
                        tokio::time::sleep(retry_delay).await;
                    }
                    retry_delay = retry_delay
                        .saturating_mul(2)
                        .min(FINALIZATION_MAX_RETRY_DELAY);
                    retry_attempt = retry_attempt.saturating_add(1);
                }
                Err(error) => return Err(DeliverNotificationError::Repository(error)),
            }
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
            let completion = DeliveryCompletion::PermanentFailure {
                error_code: "NOTIFICATION_SOURCE_MISSING",
                completed_at: OffsetDateTime::now_utc(),
            };
            self.finalize(&claimed, &completion).await?;
            return Ok(DeliverNotificationResult::SourceMissing);
        };

        match self.dispatcher.dispatch(&source).await {
            Ok(sent) => {
                let completion = DeliveryCompletion::Delivered {
                    provider_message_id: sent.provider_message_id,
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion).await?;
                Ok(DeliverNotificationResult::Delivered {
                    attempt_count: claimed.attempt_count,
                })
            }
            Err(NotificationDeliveryDispatchError::UnregisteredChannel { channel }) => {
                let completion = DeliveryCompletion::PermanentFailure {
                    error_code: UNREGISTERED_CHANNEL_ERROR_CODE,
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion).await?;
                Err(DeliverNotificationError::UnregisteredChannel { channel })
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Retryable { .. },
            )) => {
                let completion = DeliveryCompletion::RetryableFailure {
                    error_code: error.code(),
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion).await?;
                Err(DeliverNotificationError::RetryableSend(error))
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Permanent { .. },
            )) => {
                let completion = DeliveryCompletion::PermanentFailure {
                    error_code: error.code(),
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion).await?;
                Ok(DeliverNotificationResult::PermanentlyFailed)
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
    use application::error::box_error;
    use listing_source_core::ListingSourceName;
    use localization::Language;
    use notification_core::notification_id::NotificationId;
    use notification_core::{
        notification::{
            NotificationContent, PartnershipApplicationDecision,
            PartnershipApplicationNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use party_core::party_name::PartyName;
    use std::sync::{Arc, Mutex};
    use user_core::user_id::UserId;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum FinalizationCall {
        Delivered {
            lease_token: Uuid,
            provider_message_id: String,
            completed_at: OffsetDateTime,
        },
        RetryableFailure {
            lease_token: Uuid,
            error_code: String,
            completed_at: OffsetDateTime,
        },
        PermanentFailure {
            lease_token: Uuid,
            error_code: String,
            completed_at: OffsetDateTime,
        },
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    enum PersistedState {
        #[default]
        Processing,
        Delivered,
        Pending,
        Failed,
    }

    #[derive(Default)]
    struct DeliveryState {
        claimed_delivery_ids: Mutex<Vec<NotificationDeliveryId>>,
        claimed_lease_tokens: Mutex<Vec<Uuid>>,
        delivered_message_ids: Mutex<Vec<String>>,
        permanent_failure_codes: Mutex<Vec<String>>,
        finalization_calls: Mutex<Vec<FinalizationCall>>,
        finalization_failures_remaining: Mutex<usize>,
        persisted_state: Mutex<PersistedState>,
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

    fn fail_next_finalization(state: &DeliveryState) -> Result<(), NotificationDeliveryError> {
        let mut remaining = state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| repository_error())?;
        if *remaining == 0 {
            return Ok(());
        }
        *remaining -= 1;
        Err(repository_error())
    }

    #[async_trait::async_trait]
    impl NotificationDeliveryRepository for FakeDeliveryRepository {
        async fn claim_and_load_source(
            &self,
            notification_delivery_id: NotificationDeliveryId,
            _: OffsetDateTime,
            _: OffsetDateTime,
            lease_token: Uuid,
        ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
            self.state
                .claimed_delivery_ids
                .lock()
                .map_err(|_| repository_error())?
                .push(notification_delivery_id);
            self.state
                .claimed_lease_tokens
                .lock()
                .map_err(|_| repository_error())?
                .push(lease_token);
            *self
                .state
                .persisted_state
                .lock()
                .map_err(|_| repository_error())? = PersistedState::Processing;
            let mut claimed = self.claimed.clone();
            claimed.lease_token = lease_token;
            Ok(ClaimNotificationDeliveryOutcome::Claimed {
                delivery: claimed,
                source: Box::new(Some(self.source.clone())),
            })
        }

        async fn mark_delivered(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            provider_message_id: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .finalization_calls
                .lock()
                .map_err(|_| repository_error())?
                .push(FinalizationCall::Delivered {
                    lease_token,
                    provider_message_id: provider_message_id.to_owned(),
                    completed_at,
                });
            fail_next_finalization(&self.state)?;
            self.state
                .delivered_message_ids
                .lock()
                .map_err(|_| repository_error())?
                .push(provider_message_id.to_owned());
            *self
                .state
                .persisted_state
                .lock()
                .map_err(|_| repository_error())? = PersistedState::Delivered;
            Ok(true)
        }

        async fn mark_retryable_failure(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            error_code: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .finalization_calls
                .lock()
                .map_err(|_| repository_error())?
                .push(FinalizationCall::RetryableFailure {
                    lease_token,
                    error_code: error_code.to_owned(),
                    completed_at,
                });
            fail_next_finalization(&self.state)?;
            *self
                .state
                .persisted_state
                .lock()
                .map_err(|_| repository_error())? = PersistedState::Pending;
            Ok(true)
        }

        async fn mark_permanent_failure(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            error_code: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .finalization_calls
                .lock()
                .map_err(|_| repository_error())?
                .push(FinalizationCall::PermanentFailure {
                    lease_token,
                    error_code: error_code.to_owned(),
                    completed_at,
                });
            fail_next_finalization(&self.state)?;
            self.state
                .permanent_failure_codes
                .lock()
                .map_err(|_| repository_error())?
                .push(error_code.to_owned());
            *self
                .state
                .persisted_state
                .lock()
                .map_err(|_| repository_error())? = PersistedState::Failed;
            Ok(true)
        }
    }

    #[derive(Clone, Copy, Default)]
    enum SendOutcome {
        #[default]
        Delivered,
        Retryable(&'static str),
        Permanent(&'static str),
    }

    struct RecordingEmailSender {
        sent_sources: Mutex<Vec<NotificationDeliverySource>>,
        outcome: SendOutcome,
    }

    impl Default for RecordingEmailSender {
        fn default() -> Self {
            Self {
                sent_sources: Mutex::new(Vec::new()),
                outcome: SendOutcome::default(),
            }
        }
    }

    impl RecordingEmailSender {
        fn with_outcome(outcome: SendOutcome) -> Self {
            Self {
                outcome,
                ..Self::default()
            }
        }
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
            match self.outcome {
                SendOutcome::Delivered => Ok(SentNotificationDelivery {
                    provider_message_id: "provider-message-1".to_owned(),
                }),
                SendOutcome::Retryable(code) => Err(NotificationChannelSendError::Retryable {
                    code,
                    source: box_error(std::io::Error::other("test retryable provider failure")),
                }),
                SendOutcome::Permanent(code) => Err(NotificationChannelSendError::Permanent {
                    code,
                    source: box_error(std::io::Error::other("test permanent provider failure")),
                }),
            }
        }
    }

    fn source(notification_delivery_id: NotificationDeliveryId) -> NotificationDeliverySource {
        NotificationDeliverySource {
            notification_delivery_id,
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::from("Test Party"),
                    listing_source_name: ListingSourceName::from("Test Listing Source"),
                    image: None,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
            presentation_preferences: crate::presentation::NotificationPresentationPreferences {
                language: Language::En,
                show_unassessed_or_sensitive_content: false,
            },
        }
    }

    fn repository_error() -> NotificationDeliveryError {
        NotificationDeliveryError::OperationFailed {
            source: box_error(std::io::Error::other("test repository lock poisoned")),
        }
    }

    fn claimed_lease_token(
        state: &DeliveryState,
        notification_delivery_id: NotificationDeliveryId,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        let claimed_delivery_ids = state
            .claimed_delivery_ids
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        assert_eq!(vec![notification_delivery_id], claimed_delivery_ids);
        state
            .claimed_lease_tokens
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .first()
            .copied()
            .ok_or_else(|| std::io::Error::other("missing claimed lease token").into())
    }

    #[tokio::test]
    async fn should_claim_send_and_finalize_through_registered_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let source = source(notification_delivery_id);
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
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
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::Delivered {
                    lease_token: first_lease_token,
                    provider_message_id: first_provider_message_id,
                    completed_at: first_completed_at,
                },
                FinalizationCall::Delivered {
                    lease_token: second_lease_token,
                    provider_message_id: second_provider_message_id,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && first_provider_message_id == second_provider_message_id
                && first_completed_at == second_completed_at
                && first_provider_message_id == "provider-message-1"
        ));
        assert_eq!(
            PersistedState::Delivered,
            *state
                .persisted_state
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_retryable_failure_finalization_without_sending_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
        let sender = Arc::new(RecordingEmailSender::with_outcome(SendOutcome::Retryable(
            "TEST_PROVIDER_RETRYABLE",
        )));
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await;

        assert!(matches!(
            result,
            Err(DeliverNotificationError::RetryableSend(error))
                if error.code() == "TEST_PROVIDER_RETRYABLE"
        ));
        let sent_source_count = sender
            .sent_sources
            .lock()
            .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
            .len();
        assert_eq!(1, sent_source_count);
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        let claimed_lease_token = claimed_lease_token(&state, notification_delivery_id)?;
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::RetryableFailure {
                    lease_token: first_lease_token,
                    error_code: first_error_code,
                    completed_at: first_completed_at,
                },
                FinalizationCall::RetryableFailure {
                    lease_token: second_lease_token,
                    error_code: second_error_code,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && *first_lease_token == claimed_lease_token
                && first_error_code == second_error_code
                && first_completed_at == second_completed_at
                && first_error_code == "TEST_PROVIDER_RETRYABLE"
        ));
        let persisted_state = *state
            .persisted_state
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?;
        assert_eq!(PersistedState::Pending, persisted_state);
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_permanent_failure_finalization_without_sending_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
        let sender = Arc::new(RecordingEmailSender::with_outcome(SendOutcome::Permanent(
            "TEST_PROVIDER_PERMANENT",
        )));
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await?;

        assert_eq!(DeliverNotificationResult::PermanentlyFailed, result);
        let sent_source_count = sender
            .sent_sources
            .lock()
            .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
            .len();
        assert_eq!(1, sent_source_count);
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        let claimed_lease_token = claimed_lease_token(&state, notification_delivery_id)?;
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::PermanentFailure {
                    lease_token: first_lease_token,
                    error_code: first_error_code,
                    completed_at: first_completed_at,
                },
                FinalizationCall::PermanentFailure {
                    lease_token: second_lease_token,
                    error_code: second_error_code,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && *first_lease_token == claimed_lease_token
                && first_error_code == second_error_code
                && first_completed_at == second_completed_at
                && first_error_code == "TEST_PROVIDER_PERMANENT"
        ));
        let persisted_state = *state
            .persisted_state
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?;
        assert_eq!(PersistedState::Failed, persisted_state);
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
