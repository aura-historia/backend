use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;

use product_listing_postgres::SqlxProductListingWatchlistNotificationSourceReaderFactory;
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsHandler,
    GenerateWatchlistNotificationsResult, GenerateWatchlistNotificationsUseCase,
};

use std::{sync::Arc, time::Duration};
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
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_watchlist_notification_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(
                    disposition,
                    WatchlistNotificationJobDisposition::Complete(_)
                ),
                disposition.category(),
            ),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
                "Watchlist notification record retained for SQS retry or redrive");
        }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished watchlist notification batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchlistNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl WatchlistNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_watchlist_notification_job(
    body: &str,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
) -> WatchlistNotificationJobDisposition {
    match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::WatchlistNotification,
    ) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => WatchlistNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O>(
    handler: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
    job: DomainJob<O>,
) -> WatchlistNotificationJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return WatchlistNotificationJobDisposition::Poison("unexpected_payload");
    };
    // The service loads the exact historical event and locks current lifecycle through commit.
    // No transport cache/current-event comparison may suppress a later historical notification.
    match handler
        .execute(GenerateWatchlistNotificationsCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => watchlist_outcome(result),
        Err(_) => WatchlistNotificationJobDisposition::DependencyUnavailable(
            "watchlist_notification_unavailable",
        ),
    }
}

pub fn watchlist_outcome(
    result: GenerateWatchlistNotificationsResult,
) -> WatchlistNotificationJobDisposition {
    match result {
        GenerateWatchlistNotificationsResult::Applied {
            recipient_count,
            inserted_count,
            already_exists_count,
        } => {
            tracing::info!(
                recipient_count,
                inserted_count,
                already_exists_count,
                "historical watchlist notifications committed"
            );
            WatchlistNotificationJobDisposition::Complete(
                if inserted_count == 0 && already_exists_count > 0 {
                    "duplicate"
                } else {
                    "applied"
                },
            )
        }
        GenerateWatchlistNotificationsResult::SuppressedForMissingSource => {
            WatchlistNotificationJobDisposition::Retry("missing_source")
        }
        GenerateWatchlistNotificationsResult::IgnoredEvent => {
            WatchlistNotificationJobDisposition::Complete("ignored_event")
        }
        GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing => {
            WatchlistNotificationJobDisposition::Complete("withdrawn")
        }
    }
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
