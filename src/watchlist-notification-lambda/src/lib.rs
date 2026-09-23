use aura_historia_worker::watchlist_notifications::{
    WatchlistNotificationJobDisposition, process_watchlist_notification_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use lambda_runtime::{Error, LambdaEvent};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_postgres::SqlxUnitOfWork;

use product_listing_postgres::SqlxProductListingWatchlistNotificationSourceReaderFactory;
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsUseCase,
};

use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the PostgreSQL-backed watchlist notification use case used by production.
pub fn compose_watchlist_notification_use_case(
    pool: sqlx::PgPool,
) -> Arc<dyn GenerateWatchlistNotificationsUseCase> {
    Arc::new(GenerateWatchlistNotificationsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingWatchlistNotificationSourceReaderFactory::new(),
        SqlxWatchlistNotificationRecipientReaderFactory,
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    ))
}

/// Uses a single deadline for credential refresh, composition, record work, and Lambda response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
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

/// Returns every record as unfinished when startup work consumes the usable invocation budget.
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
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
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
        if processing_budget.is_zero() {
            warn!(message_id = %message_id, outcome = "insufficient_invocation_budget", "Watchlist notification record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
            continue;
        }
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => WatchlistNotificationJobDisposition::Poison("missing_message_body"),
        };
        if !matches!(
            disposition,
            WatchlistNotificationJobDisposition::Complete(_)
        ) {
            warn!(message_id = %message_id, outcome = disposition.category(), "Watchlist notification record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished watchlist notification batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
    processing_budget: Duration,
) -> WatchlistNotificationJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_watchlist_notification_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => WatchlistNotificationJobDisposition::Retry("execution_timeout"),
        Err(_) => WatchlistNotificationJobDisposition::Retry("handler_panicked"),
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
    use product_listing_service::use_cases::{
        GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsError,
        GenerateWatchlistNotificationsResult,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    #[tokio::test]
    async fn acknowledges_completed_watchlist_results_but_retains_retry_and_poison_records() {
        let use_case = FakeUseCase::new([
            FakeResult::Applied,
            FakeResult::Ignored,
            FakeResult::Withdrawn,
            FakeResult::MissingSource,
            FakeResult::Unavailable,
        ]);
        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("ignored"), valid_body()),
                (Some("withdrawn"), valid_body()),
                (Some("retry"), valid_body()),
                (Some("unavailable"), valid_body()),
                (Some("poison"), "not-json".to_owned()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(failure_ids(response), ["retry", "unavailable", "poison"]);
        assert_eq!(use_case.calls(), 5);
    }

    #[tokio::test]
    async fn retains_timed_out_and_unstarted_records_after_budget_is_spent() {
        let use_case = FakeUseCase::new([FakeResult::Pending, FakeResult::Applied]);
        let response = handler_with_budget(
            events([
                (Some("timed-out"), valid_body()),
                (Some("unstarted"), valid_body()),
            ]),
            &use_case,
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .await
        .expect("handler response");

        assert_eq!(failure_ids(response), ["timed-out", "unstarted"]);
        assert_eq!(use_case.calls(), 1);
    }

    #[test]
    fn retains_all_records_when_setup_cannot_complete() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial batch response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    fn valid_body() -> String {
        r#"{"schema_version":2,"scope":"watchlist-notification","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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

    #[derive(Clone, Copy)]
    enum FakeResult {
        Applied,
        Ignored,
        Withdrawn,
        MissingSource,
        Unavailable,
        Pending,
    }

    struct FakeUseCase {
        results: Mutex<VecDeque<FakeResult>>,
        calls: AtomicUsize,
    }

    impl FakeUseCase {
        fn new<const N: usize>(results: [FakeResult; N]) -> Self {
            Self {
                results: Mutex::new(VecDeque::from(results)),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    #[async_trait::async_trait]
    impl GenerateWatchlistNotificationsUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: GenerateWatchlistNotificationsCommand,
        ) -> Result<GenerateWatchlistNotificationsResult, GenerateWatchlistNotificationsError>
        {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = { self.results.lock().expect("results lock").pop_front() };
            match result {
                Some(FakeResult::Applied) => Ok(GenerateWatchlistNotificationsResult::Applied {
                    recipient_count: 1,
                    inserted_count: 1,
                    already_exists_count: 0,
                }),
                Some(FakeResult::Ignored) => Ok(GenerateWatchlistNotificationsResult::IgnoredEvent),
                Some(FakeResult::Withdrawn) => {
                    Ok(GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing)
                }
                Some(FakeResult::MissingSource) => {
                    Ok(GenerateWatchlistNotificationsResult::SuppressedForMissingSource)
                }
                Some(FakeResult::Unavailable) => Err(
                    GenerateWatchlistNotificationsError::BeginTransactionFailed {
                        source: Box::new(std::io::Error::other("unavailable")),
                    },
                ),
                Some(FakeResult::Pending) => std::future::pending().await,
                None => Ok(GenerateWatchlistNotificationsResult::IgnoredEvent),
            }
        }
    }
}
