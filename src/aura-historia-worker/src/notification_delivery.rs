use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use notification_service::{
    ports::notification_delivery_repository::NotificationDeliveryError,
    use_cases::commands::deliver_notification::{
        DeliverNotificationCommand, DeliverNotificationError, DeliverNotificationResult,
        DeliverNotificationUseCase,
    },
};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tracing::{info, warn};

const LEASE_SAFETY: Duration = Duration::seconds(5);

pub async fn consume_notification_delivery_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn DeliverNotificationUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::NotificationDelivery, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}
async fn execute_job(use_case: Arc<dyn DeliverNotificationUseCase>, job: DomainJob) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("delivery_metadata_invalid");
    };
    match use_case.execute(command).await {
        Ok(result) => delivery_outcome(result, OffsetDateTime::now_utc()),
        Err(error) => delivery_error(error),
    }
}

/// Lambda- and polling-transport result for one compact notification-delivery wake-up.
///
/// Only durable terminal service results are complete. All other dispositions deliberately leave
/// the SQS record unfinished: an accepted SES send cannot be atomically finalized in PostgreSQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl NotificationDeliveryJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

/// Decode and execute one compact schema-2 delivery wake-up without exposing SQS transport
/// metadata to the notification service. Lease ownership and finalization remain PostgreSQL-owned.
pub async fn process_notification_delivery_job(
    body: &str,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
) -> NotificationDeliveryJobDisposition {
    let job = match crate::wire::decode(body, WorkerScope::NotificationDelivery) {
        Ok(job) => job,
        Err(_) => return NotificationDeliveryJobDisposition::Poison("invalid_wire_job"),
    };
    let command = match command_from_job(job) {
        Ok(command) => command,
        Err(_) => return NotificationDeliveryJobDisposition::Poison("unexpected_payload"),
    };
    let notification_delivery_id = command.notification_delivery_id;
    match use_case.execute(command).await {
        Ok(result) => {
            let attempt_count = match result {
                DeliverNotificationResult::Delivered { attempt_count } => Some(attempt_count),
                _ => None,
            };
            let disposition = delivery_disposition(result);
            info!(
                notification_delivery_id = %notification_delivery_id,
                attempt_count,
                outcome = disposition.category(),
                "notification delivery attempt finished"
            );
            disposition
        }
        Err(error) => {
            let disposition = delivery_error_disposition(error);
            warn!(
                notification_delivery_id = %notification_delivery_id,
                outcome = disposition.category(),
                "notification delivery attempt remains unfinished"
            );
            disposition
        }
    }
}

fn delivery_disposition(result: DeliverNotificationResult) -> NotificationDeliveryJobDisposition {
    match result {
        DeliverNotificationResult::Delivered { .. } => {
            NotificationDeliveryJobDisposition::Complete("delivered")
        }
        DeliverNotificationResult::AlreadyDelivered => {
            NotificationDeliveryJobDisposition::Complete("already_delivered")
        }
        DeliverNotificationResult::PermanentlyFailed => {
            NotificationDeliveryJobDisposition::Complete("permanently_failed")
        }
        DeliverNotificationResult::SourceMissing => {
            NotificationDeliveryJobDisposition::Complete("source_missing_finalized")
        }
        DeliverNotificationResult::DeliveryMissing => {
            NotificationDeliveryJobDisposition::Retry("delivery_missing")
        }
        DeliverNotificationResult::AlreadyClaimed { .. } => {
            NotificationDeliveryJobDisposition::Retry("already_claimed")
        }
        DeliverNotificationResult::ClaimDeferred { .. } => {
            NotificationDeliveryJobDisposition::Retry("claim_deferred")
        }
    }
}

fn delivery_error_disposition(
    error: DeliverNotificationError,
) -> NotificationDeliveryJobDisposition {
    match error {
        DeliverNotificationError::Repository(
            NotificationDeliveryError::InvalidPersistedState { .. },
        ) => NotificationDeliveryJobDisposition::Poison("delivery_state_invalid"),
        DeliverNotificationError::UnregisteredChannel { .. } => {
            NotificationDeliveryJobDisposition::Poison("delivery_channel_unregistered")
        }
        DeliverNotificationError::LeaseLost => {
            NotificationDeliveryJobDisposition::Retry("lease_lost")
        }
        DeliverNotificationError::AmbiguousSend(_) => {
            NotificationDeliveryJobDisposition::DependencyUnavailable("provider_acceptance_unknown")
        }
        DeliverNotificationError::AttemptTimedOut { .. } => {
            NotificationDeliveryJobDisposition::DependencyUnavailable("delivery_attempt_timeout")
        }
        DeliverNotificationError::FinalizationExhausted { .. } => {
            NotificationDeliveryJobDisposition::DependencyUnavailable(
                "delivery_finalization_unconfirmed",
            )
        }
        DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed {
            ..
        })
        | DeliverNotificationError::RetryableSend(_) => {
            NotificationDeliveryJobDisposition::DependencyUnavailable(
                "delivery_dependency_unavailable",
            )
        }
    }
}
fn delivery_outcome(result: DeliverNotificationResult, now: OffsetDateTime) -> JobOutcome {
    match result {
        DeliverNotificationResult::Delivered { .. } => JobOutcome::Complete("delivered"),
        DeliverNotificationResult::AlreadyDelivered => JobOutcome::Complete("already_delivered"),
        DeliverNotificationResult::PermanentlyFailed => JobOutcome::Complete("permanently_failed"),
        // This result follows a committed permanent-failure finalization in the service.
        DeliverNotificationResult::SourceMissing => {
            JobOutcome::Complete("source_missing_finalized")
        }
        DeliverNotificationResult::DeliveryMissing => JobOutcome::Retry("delivery_missing"),
        DeliverNotificationResult::AlreadyClaimed { lease_expires_at } => {
            match lease_expires_at.checked_add(LEASE_SAFETY) {
                Some(not_before) => JobOutcome::RetryAfter(not_before),
                None => JobOutcome::Invalid("lease_expiry_invalid"),
            }
        }
        DeliverNotificationResult::ClaimDeferred { retry_after } => {
            let Ok(delay) = Duration::try_from(retry_after) else {
                return JobOutcome::Invalid("claim_delay_invalid");
            };
            match now.checked_add(delay.max(Duration::seconds(1))) {
                Some(not_before) => JobOutcome::RetryAfter(not_before),
                None => JobOutcome::Invalid("claim_delay_invalid"),
            }
        }
    }
}
fn delivery_error(error: DeliverNotificationError) -> JobOutcome {
    match error {
        DeliverNotificationError::Repository(
            NotificationDeliveryError::InvalidPersistedState { .. },
        ) => JobOutcome::Invalid("delivery_state_invalid"),
        DeliverNotificationError::UnregisteredChannel { .. } => {
            JobOutcome::Invalid("delivery_channel_unregistered")
        }
        DeliverNotificationError::LeaseLost => JobOutcome::Retry("lease_lost"),
        DeliverNotificationError::AmbiguousSend(_) => {
            JobOutcome::DependencyUnavailable("provider_acceptance_unknown")
        }
        DeliverNotificationError::AttemptTimedOut { .. } => {
            JobOutcome::DependencyUnavailable("delivery_attempt_timeout")
        }
        DeliverNotificationError::FinalizationExhausted { .. } => {
            JobOutcome::DependencyUnavailable("delivery_finalization_unconfirmed")
        }
        DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed {
            ..
        })
        | DeliverNotificationError::RetryableSend(_) => {
            JobOutcome::DependencyUnavailable("delivery_dependency_unavailable")
        }
    }
}
fn command_from_job(job: DomainJob) -> Result<DeliverNotificationCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::NotificationDeliveryCreated(delivery) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(DeliverNotificationCommand {
        notification_delivery_id: delivery.notification_delivery_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QueueConfig,
        cdc::{IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey, WorkerQueue},
        in_memory_queue,
    };
    use notification_core::notification_delivery_id::NotificationDeliveryId;
    use std::sync::Mutex;
    use uuid::Uuid;
    #[derive(Default)]
    struct Handler {
        commands: Mutex<Vec<DeliverNotificationCommand>>,
    }
    #[async_trait::async_trait]
    impl DeliverNotificationUseCase for Handler {
        async fn execute(
            &self,
            command: DeliverNotificationCommand,
        ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
            self.commands
                .lock()
                .map_err(|_| DeliverNotificationError::LeaseLost)?
                .push(command);
            Ok(DeliverNotificationResult::Delivered { attempt_count: 1 })
        }
    }
    #[tokio::test]
    async fn should_map_notification_delivery_job_to_delivery_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(1))?;
        let notification_delivery_id = NotificationDeliveryId::try_from(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0007,
        ))?;
        assert_eq!(
            "nd_01j0000000e008000000000007",
            notification_delivery_id.to_string()
        );
        sender
            .enqueue(DomainJob {
                target_queue: WorkerQueue::NotificationDelivery,
                idempotency_key: IdempotencyKey::new(format!(
                    "notification-delivery:{notification_delivery_id}"
                )),
                ordering_key: OrderingKey::new(format!(
                    "notification-delivery:{notification_delivery_id}"
                )),
                payload: DomainJobPayload::NotificationDeliveryCreated(
                    NotificationDeliveryCreatedJob {
                        notification_delivery_id,
                    },
                ),
            })
            .await?;
        drop(sender);
        let handler = Arc::new(Handler::default());
        consume_notification_delivery_queue(receiver, handler.clone()).await;
        assert_eq!(
            vec![DeliverNotificationCommand {
                notification_delivery_id
            }],
            *handler.commands.lock().unwrap()
        );
        Ok(())
    }
    #[test]
    fn should_defer_active_lease_to_persisted_expiry_plus_safety() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let expiry = now + Duration::minutes(5);
        assert_eq!(
            JobOutcome::RetryAfter(expiry + LEASE_SAFETY),
            delivery_outcome(
                DeliverNotificationResult::AlreadyClaimed {
                    lease_expires_at: expiry
                },
                now
            )
        );
        assert_eq!(
            JobOutcome::RetryAfter(now + Duration::seconds(2)),
            delivery_outcome(
                DeliverNotificationResult::ClaimDeferred {
                    retry_after: std::time::Duration::from_secs(2)
                },
                now
            )
        );
    }
    #[test]
    fn should_never_ack_delivery_errors_in_any_attempt_phase() {
        use notification_core::notification_delivery::NotificationDeliveryChannel;
        use notification_service::{
            ports::notification_channel_sender::NotificationChannelSendError,
            use_cases::commands::deliver_notification::DeliveryAttemptPhase,
        };
        for error in [
            DeliverNotificationError::LeaseLost,
            DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed {
                source: std::io::Error::other("database unavailable").into(),
            }),
            DeliverNotificationError::Repository(
                NotificationDeliveryError::InvalidPersistedState {
                    source: std::io::Error::other("invalid persisted state").into(),
                },
            ),
            DeliverNotificationError::UnregisteredChannel {
                channel: NotificationDeliveryChannel::Email,
            },
            DeliverNotificationError::RetryableSend(NotificationChannelSendError::Retryable {
                code: "UNAVAILABLE",
                source: std::io::Error::other("provider unavailable").into(),
            }),
            DeliverNotificationError::AmbiguousSend(NotificationChannelSendError::Ambiguous {
                code: "UNKNOWN",
                source: std::io::Error::other("response lost").into(),
            }),
            DeliverNotificationError::FinalizationExhausted {
                source: NotificationDeliveryError::OperationFailed {
                    source: std::io::Error::other("commit unconfirmed").into(),
                },
            },
        ] {
            assert!(!matches!(delivery_error(error), JobOutcome::Complete(_)));
        }
        for phase in [
            DeliveryAttemptPhase::Claim,
            DeliveryAttemptPhase::Provider,
            DeliveryAttemptPhase::Finalization,
        ] {
            assert!(!matches!(
                delivery_error(DeliverNotificationError::AttemptTimedOut { phase }),
                JobOutcome::Complete(_)
            ));
        }
    }

    #[test]
    fn should_bound_deferrals_without_treating_invalid_expiry_as_completion() {
        let now = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(
            JobOutcome::RetryAfter(now + Duration::seconds(1)),
            delivery_outcome(
                DeliverNotificationResult::ClaimDeferred {
                    retry_after: std::time::Duration::ZERO
                },
                now
            )
        );
        assert!(matches!(
            delivery_outcome(
                DeliverNotificationResult::ClaimDeferred {
                    retry_after: std::time::Duration::MAX,
                },
                now
            ),
            JobOutcome::Invalid(_)
        ));
        let expiry = time::Date::MAX.with_hms(23, 59, 59).unwrap().assume_utc();
        assert!(matches!(
            delivery_outcome(
                DeliverNotificationResult::AlreadyClaimed {
                    lease_expires_at: expiry,
                },
                now
            ),
            JobOutcome::Invalid(_)
        ));
    }

    #[test]
    fn should_delete_only_durable_terminal_delivery_results() {
        let now = OffsetDateTime::UNIX_EPOCH;
        for result in [
            DeliverNotificationResult::Delivered { attempt_count: 1 },
            DeliverNotificationResult::AlreadyDelivered,
            DeliverNotificationResult::SourceMissing,
            DeliverNotificationResult::PermanentlyFailed,
        ] {
            assert!(matches!(
                delivery_outcome(result, now),
                JobOutcome::Complete(_)
            ));
        }
        assert!(matches!(
            delivery_outcome(DeliverNotificationResult::DeliveryMissing, now),
            JobOutcome::Retry(_)
        ));
        for error in [DeliverNotificationError::LeaseLost,
            DeliverNotificationError::AttemptTimedOut { phase: notification_service::use_cases::commands::deliver_notification::DeliveryAttemptPhase::Finalization }] {
            assert!(!matches!(delivery_error(error), JobOutcome::Complete(_)));
        }
    }
}
