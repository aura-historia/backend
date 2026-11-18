//! `watchlist-notification`: creates the watchlist notifications for one historical
//! ProductListing event. The use case is PostgreSQL-only.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::SqlxProductListingWatchlistNotificationSourceReaderFactory;
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsError,
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsResult,
    GenerateWatchlistNotificationsUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::warn;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

/// Builds the PostgreSQL-backed watchlist notification use case used by production.
pub fn compose_watchlist_notification_use_case(
    pool: PgPool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchlistNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl WatchlistNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "watchlist_notification_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_watchlist_notification_job(
    body: &str,
    use_case: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
) -> WatchlistNotificationJobDisposition {
    match decode(body, WorkerScope::WatchlistNotification) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => WatchlistNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    handler: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
    job: DomainJob,
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
        Err(error) => watchlist_error_disposition(&error),
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn watchlist_error_disposition(
    error: &GenerateWatchlistNotificationsError,
) -> WatchlistNotificationJobDisposition {
    use GenerateWatchlistNotificationsError as E;
    let stage = match error {
        E::BeginTransactionFailed { .. } => "source_begin",
        E::SourceReadFailed { .. } => "source_read",
        E::RecipientReadFailed { .. } => "recipient_read",
        E::CommitTransactionFailed { .. } => "source_commit",
        E::NotificationCreateFailed { .. } => "notification_create",
    };
    WatchlistNotificationJobDisposition::DependencyUnavailable(FailureDiagnostic::postgres(
        stage, error,
    ))
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

/// Only completion acknowledges; every other outcome, including poison, is retried so the
/// queue's attempt bound, not the handler, decides failure custody.
pub fn disposition(job: &Job, outcome: WatchlistNotificationJobDisposition) -> Disposition {
    if let WatchlistNotificationJobDisposition::Complete(_) = outcome {
        return Disposition::Complete;
    }
    let category = outcome.category();
    let failure = outcome.failure_diagnostic();
    warn!(
        job_id = %job.id(),
        attempt = job.attempt(),
        outcome = category,
        stage = failure.map(FailureDiagnostic::stage),
        dependency = failure.and_then(FailureDiagnostic::dependency),
        error_category = failure.map(FailureDiagnostic::category),
        http_status = failure.and_then(FailureDiagnostic::http_status),
        sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
        "Watchlist notification record retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct WatchlistNotificationJobs {
    use_case: Arc<dyn GenerateWatchlistNotificationsUseCase>,
}

impl WatchlistNotificationJobs {
    pub fn new(use_case: Arc<dyn GenerateWatchlistNotificationsUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for WatchlistNotificationJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_watchlist_notification_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(WatchlistNotificationJobs::new(
        compose_watchlist_notification_use_case(pool.clone()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    #[tokio::test]
    async fn acknowledges_completed_watchlist_results_but_retains_retry_and_poison_records() {
        let cases = [
            (FakeResult::Applied, Disposition::Complete),
            (FakeResult::Duplicate, Disposition::Complete),
            (FakeResult::Ignored, Disposition::Complete),
            (FakeResult::Withdrawn, Disposition::Complete),
            (
                FakeResult::MissingSource,
                Disposition::Retry(error_class("missing_source")),
            ),
            (
                FakeResult::Unavailable,
                Disposition::Retry(error_class("watchlist_notification_unavailable")),
            ),
        ];
        let (results, expected): (Vec<_>, Vec<_>) = cases.into_iter().unzip();
        let use_case = Arc::new(FakeUseCase::new(results));
        let handler = WatchlistNotificationJobs::new(use_case.clone());
        let mut actual = Vec::new();
        for _ in 0..expected.len() {
            actual.push(handler.handle(&claimed_job(&valid_body())).await);
        }
        assert_eq!(expected, actual);

        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job("not-json")).await
        );
        assert_eq!(use_case.calls(), 6);
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let handler =
            WatchlistNotificationJobs::new(Arc::new(FakeUseCase::new([FakeResult::Unavailable])));
        let job = claimed_job(&valid_body());
        let (logs, guard) = CapturedLogs::start();

        let disposition = handler.handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("watchlist_notification_unavailable")),
            disposition
        );
        let retained = logs.events_where(
            "message",
            "Watchlist notification record retained for retry",
        );
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("source_begin"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("protocol"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"watchlist-notification","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Applied,
        Duplicate,
        Ignored,
        Withdrawn,
        MissingSource,
        Unavailable,
    }

    struct FakeUseCase {
        results: Mutex<VecDeque<FakeResult>>,
        calls: AtomicUsize,
    }

    impl FakeUseCase {
        fn new(results: impl IntoIterator<Item = FakeResult>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
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
                Some(FakeResult::Duplicate) => Ok(GenerateWatchlistNotificationsResult::Applied {
                    recipient_count: 1,
                    inserted_count: 0,
                    already_exists_count: 1,
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
                        source: Box::new(sqlx::Error::Protocol(SENTINEL.to_owned())),
                    },
                ),
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
