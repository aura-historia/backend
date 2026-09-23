use aura_historia_worker::notification_delivery::{
    NotificationDeliveryJobDisposition, process_notification_delivery_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use lambda_runtime::{Error, LambdaEvent};
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryRepository};
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSender, NotificationDeliveryDispatcher,
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationTiming, DeliverNotificationUseCase,
    },
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
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
    let failures = event
        .payload
        .records
        .iter()
        .map(|record| {
            record.message_id.clone().ok_or_else(|| {
                Error::from("SQS event record has no message ID; fail whole invocation")
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(batch_failure)
        .collect();
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let record_count = event.payload.records.len();
    let invocation_started_at = Instant::now();
    let mut failures = Vec::new();

    for record in event.payload.records {
        let message_id = record.message_id.ok_or_else(|| {
            Error::from("SQS event record has no message ID; fail whole invocation")
        })?;
        let remaining = invocation_budget.saturating_sub(invocation_started_at.elapsed());
        let processing_budget = max_record_processing_budget.min(remaining);
        let disposition = if processing_budget.is_zero() {
            NotificationDeliveryJobDisposition::Retry("insufficient_invocation_budget")
        } else {
            match record.body {
                Some(body) => process_with_budget(&body, use_case, processing_budget).await,
                None => NotificationDeliveryJobDisposition::Poison("missing_message_body"),
            }
        };
        if !matches!(disposition, NotificationDeliveryJobDisposition::Complete(_)) {
            warn!(message_id = %message_id, outcome = disposition.category(), "Notification delivery record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished notification delivery batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn DeliverNotificationUseCase + Send + Sync),
    processing_budget: Duration,
) -> NotificationDeliveryJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_notification_delivery_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => NotificationDeliveryJobDisposition::Retry("execution_timeout"),
        Err(_) => NotificationDeliveryJobDisposition::Retry("handler_panicked"),
    }
}

fn batch_failure(message_id: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = message_id;
    failure
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
