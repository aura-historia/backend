use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryRepository};
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSender, NotificationDeliveryDispatcher,
    },
    ports::notification_delivery_repository::NotificationDeliveryError,
    use_cases::commands::deliver_notification::{
        DeliverNotificationCommand, DeliverNotificationError, DeliverNotificationHandler,
        DeliverNotificationResult, DeliverNotificationTiming, DeliverNotificationUseCase,
    },
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

pub const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);
const DELIVERY_ATTEMPT_BUDGET: Duration = Duration::from_secs(35);
const DELIVERY_OPERATION_BUDGET: Duration = Duration::from_secs(30);

pub fn compose_delivery_use_case(
    pool: sqlx::PgPool,
    s3: aws_sdk_s3::Client,
    ses: aws_sdk_sesv2::Client,
    email: EmailDeliveryConfig,
) -> Result<Arc<dyn DeliverNotificationUseCase>, Error> {
    let dispatcher =
        NotificationDeliveryDispatcher::new(vec![Arc::new(SesNotificationChannelSender::new(
            s3,
            ses,
            email,
            Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
        )) as Arc<dyn NotificationChannelSender>])
        .map_err(|_| Error::from("notification delivery dispatcher configuration is invalid"))?;
    dispatcher
        .validate_channels([NotificationDeliveryChannel::Email])
        .map_err(|_| Error::from("EMAIL notification channel is not registered"))?;
    let timing = DeliverNotificationTiming::new(
        DELIVERY_ATTEMPT_BUDGET,
        DELIVERY_OPERATION_BUDGET,
        Duration::from_millis(100),
        Duration::from_secs(5),
    )
    .map_err(|_| Error::from("notification delivery timing is invalid"))?;
    Ok(Arc::new(DeliverNotificationHandler::with_timing(
        SqlxNotificationDeliveryRepository::new(pool),
        dispatcher,
        timing,
    )))
}

/// Uses a single deadline for credential refresh, composition, record work, and Lambda response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
    budget: &LambdaInvocationBudget,
) -> Result<SqsBatchResponse, Error> {
    handler_with_budget(
        event,
        use_case,
        budget.remaining(),
        MAX_RECORD_PROCESSING_BUDGET,
    )
    .await
}

/// Returns every record as unfinished when setup consumes the usable invocation budget.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_notification_delivery_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(disposition, NotificationDeliveryJobDisposition::Complete(_)),
                disposition.category(),
            ),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
                "Notification delivery record retained for SQS retry or redrive");
        }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished notification delivery batch"
    );
    Ok(response)
}

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
    let job = match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::NotificationDelivery,
    ) {
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

pub fn delivery_disposition(
    result: DeliverNotificationResult,
) -> NotificationDeliveryJobDisposition {
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

pub fn delivery_error_disposition(
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
pub fn command_from_job<O>(
    job: DomainJob<O>,
) -> Result<DeliverNotificationCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::NotificationDeliveryCreated(delivery) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(DeliverNotificationCommand {
        notification_delivery_id: delivery.notification_delivery_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;

    use notification_service::use_cases::commands::deliver_notification::{
        DeliverNotificationCommand, DeliverNotificationError, DeliverNotificationResult,
    };
    use std::{
        collections::VecDeque,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[tokio::test]
    async fn acknowledges_only_durable_terminal_delivery_results() {
        let use_case = FakeUseCase::new([
            Ok(DeliverNotificationResult::Delivered { attempt_count: 1 }),
            Ok(DeliverNotificationResult::AlreadyDelivered),
            Ok(DeliverNotificationResult::PermanentlyFailed),
            Ok(DeliverNotificationResult::SourceMissing),
            Ok(DeliverNotificationResult::DeliveryMissing),
            Ok(DeliverNotificationResult::ClaimDeferred {
                retry_after: Duration::from_secs(1),
            }),
            Err(DeliverNotificationError::LeaseLost),
        ]);
        let response = handler(
            events([
                (Some("delivered"), valid_body()),
                (Some("already-delivered"), valid_body()),
                (Some("permanent"), valid_body()),
                (Some("source-missing"), valid_body()),
                (Some("missing"), valid_body()),
                (Some("deferred"), valid_body()),
                (Some("lease-lost"), valid_body()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(response), ["missing", "deferred", "lease-lost"]);
    }

    #[tokio::test]
    async fn retains_invalid_unknown_and_timed_out_work() {
        let use_case = FakeUseCase::new([Ok(DeliverNotificationResult::Delivered {
            attempt_count: 1,
        })]);
        let malformed = handler(
            events([(Some("malformed"), "not-json".to_owned())]),
            &use_case,
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(malformed), ["malformed"]);

        let timeout = handler_with_budget(
            events([(Some("timeout"), valid_body())]),
            &PendingUseCase,
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(timeout), ["timeout"]);
    }

    #[test]
    fn retains_every_record_when_setup_cannot_complete() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial batch response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    fn valid_body() -> String {
        r#"{"schema_version":2,"scope":"notification-delivery","job_type":"NOTIFICATION_DELIVERY_CREATED","idempotency_key":"notification-delivery:nd_01j0000000e008000000000007","ordering_key":"notification-delivery:nd_01j0000000e008000000000007","payload":{"notification_delivery_id":"nd_01j0000000e008000000000007"}}"#.to_owned()
    }

    fn events<const N: usize>(records: [(Option<&str>, String); N]) -> LambdaEvent<SqsEvent> {
        let mut event = SqsEvent::default();
        event.records = records
            .into_iter()
            .map(|(message_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = message_id.map(ToOwned::to_owned);
                message.body = Some(body);
                message
            })
            .collect();
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(60_000);
        LambdaEvent::new(event, context)
    }

    fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    fn epoch_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    struct FakeUseCase {
        results: Mutex<VecDeque<Result<DeliverNotificationResult, DeliverNotificationError>>>,
    }

    impl FakeUseCase {
        fn new<const N: usize>(
            results: [Result<DeliverNotificationResult, DeliverNotificationError>; N],
        ) -> Self {
            Self {
                results: Mutex::new(VecDeque::from(results)),
            }
        }
    }

    #[async_trait::async_trait]
    impl DeliverNotificationUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: DeliverNotificationCommand,
        ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
            self.results
                .lock()
                .expect("results lock")
                .pop_front()
                .unwrap_or(Ok(DeliverNotificationResult::AlreadyDelivered))
        }
    }

    struct PendingUseCase;

    #[async_trait::async_trait]
    impl DeliverNotificationUseCase for PendingUseCase {
        async fn execute(
            &self,
            _: DeliverNotificationCommand,
        ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
            std::future::pending().await
        }
    }
}
