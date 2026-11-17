//! `search-filter-match-notification`: creates the notification for one committed saved-filter
//! match. The use case is PostgreSQL-only.

use crate::composition::CompositionError;
use application::error::FailureKind;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
    SqlxSavedSearchDigestStoreFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_postgres::{
    SqlxSearchFilterMatchNotificationSourceReaderFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationResult,
    GenerateSearchFilterMatchNotificationUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::warn;
use user_postgres::SqlxUserTierEntitlementsFactory;

/// Builds the PostgreSQL-only notification use case.
pub fn compose_search_filter_match_notification_use_case(
    pool: PgPool,
) -> Arc<dyn GenerateSearchFilterMatchNotificationUseCase> {
    Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterMatchNotificationSourceReaderFactory,
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
        SqlxProductListingContentAssessmentSnapshotReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
        SqlxSavedSearchDigestStoreFactory,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterMatchNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl SearchFilterMatchNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "match_notification_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_search_filter_match_notification_job(
    body: &str,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
) -> SearchFilterMatchNotificationJobDisposition {
    match decode(body, WorkerScope::SearchFilterMatchNotification) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterMatchNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
    job: DomainJob,
) -> SearchFilterMatchNotificationJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return SearchFilterMatchNotificationJobDisposition::Poison("match_metadata_invalid");
    };
    match use_case.execute(command).await {
        Ok(result) => notification_outcome(result),
        Err(error) => notification_error_disposition(&error),
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn notification_error_disposition(
    error: &GenerateSearchFilterMatchNotificationError,
) -> SearchFilterMatchNotificationJobDisposition {
    match error.failure_kind() {
        FailureKind::InvalidPersistedState => {
            SearchFilterMatchNotificationJobDisposition::Poison("match_notification_state_invalid")
        }
        kind => SearchFilterMatchNotificationJobDisposition::DependencyUnavailable(
            FailureDiagnostic::from_kind(error.failure_stage(), kind, None, error),
        ),
    }
}

pub fn notification_outcome(
    result: GenerateSearchFilterMatchNotificationResult,
) -> SearchFilterMatchNotificationJobDisposition {
    use GenerateSearchFilterMatchNotificationResult as R;
    match result {
        R::Created => SearchFilterMatchNotificationJobDisposition::Complete("inserted"),
        R::AlreadyExists => SearchFilterMatchNotificationJobDisposition::Complete("duplicate"),
        R::SuppressedByQuota => {
            SearchFilterMatchNotificationJobDisposition::Complete("suppressed_by_quota")
        }
        // User deletion is a terminal recipient suppression, not missing historical business truth.
        R::SuppressedForMissingUser => {
            SearchFilterMatchNotificationJobDisposition::Complete("missing_user")
        }
        R::SuppressedForWithdrawnProductListing => {
            SearchFilterMatchNotificationJobDisposition::Complete("withdrawn")
        }
        R::SuppressedForStaleMatch => {
            SearchFilterMatchNotificationJobDisposition::Complete("stale_match")
        }
        R::SuppressedForMissingMatch => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_match")
        }
        R::SuppressedForMissingProductListing => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_product")
        }
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<GenerateSearchFilterMatchNotificationCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(GenerateSearchFilterMatchNotificationCommand {
        user_id: change.user_id,
        search_filter_id: change.user_search_filter_id,
        product_listing_id: change.product_listing_id,
        origin_event_id: change.origin_event_id,
    })
}

/// Only semantic completion acknowledges; every other outcome, including poison, is retried so
/// the queue's attempt bound, not the handler, decides failure custody.
pub fn disposition(job: &Job, outcome: SearchFilterMatchNotificationJobDisposition) -> Disposition {
    if let SearchFilterMatchNotificationJobDisposition::Complete(_) = outcome {
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
        "Saved-filter match-notification record retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct SearchFilterMatchNotificationJobs {
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
}

impl SearchFilterMatchNotificationJobs {
    pub fn new(use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for SearchFilterMatchNotificationJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_search_filter_match_notification_job(job.payload(), self.use_case.as_ref())
                .await;
        disposition(job, outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(SearchFilterMatchNotificationJobs::new(
        compose_search_filter_match_notification_use_case(pool.clone()),
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
    async fn acknowledges_semantic_completion_and_retries_unfinished_or_poisoned_records() {
        use GenerateSearchFilterMatchNotificationResult as R;
        let cases = [
            (FakeResult::Result(R::Created), Disposition::Complete),
            (FakeResult::Result(R::AlreadyExists), Disposition::Complete),
            (
                FakeResult::Result(R::SuppressedByQuota),
                Disposition::Complete,
            ),
            (
                FakeResult::Result(R::SuppressedForMissingUser),
                Disposition::Complete,
            ),
            (
                FakeResult::Result(R::SuppressedForWithdrawnProductListing),
                Disposition::Complete,
            ),
            (
                FakeResult::Result(R::SuppressedForStaleMatch),
                Disposition::Complete,
            ),
            (
                FakeResult::Result(R::SuppressedForMissingMatch),
                Disposition::Retry(error_class("missing_match")),
            ),
            (
                FakeResult::Result(R::SuppressedForMissingProductListing),
                Disposition::Retry(error_class("missing_product")),
            ),
            (
                FakeResult::StateInvalid,
                Disposition::Retry(error_class("match_notification_state_invalid")),
            ),
            (
                FakeResult::Unavailable,
                Disposition::Retry(error_class("match_notification_unavailable")),
            ),
        ];
        let (results, expected): (Vec<_>, Vec<_>) = cases.into_iter().unzip();
        let use_case = Arc::new(FakeUseCase::new(results));
        let handler = SearchFilterMatchNotificationJobs::new(use_case.clone());
        let mut actual = Vec::new();
        for _ in 0..expected.len() {
            actual.push(handler.handle(&claimed_job(&valid_body())).await);
        }
        assert_eq!(expected, actual);

        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job("not-json")).await
        );
        assert_eq!(use_case.calls(), 10);
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let handler = SearchFilterMatchNotificationJobs::new(Arc::new(FakeUseCase::new([
            FakeResult::Unavailable,
        ])));
        let job = claimed_job(&valid_body());
        let (logs, guard) = CapturedLogs::start();

        let disposition = handler.handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("match_notification_unavailable")),
            disposition
        );
        let retained = logs.events_where(
            "message",
            "Saved-filter match-notification record retained for retry",
        );
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("match_source_read"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("tls"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"search-filter-match-notification","job_type":"SEARCH_FILTER_MATCH_CREATED","idempotency_key":"search-filter-match:usr_01h455vb4pex5vy7enb1p677vn:sf_01h455vb4pex5vy7enb1p677vn:pl_01h455vb4pex5vy7enb1p677vn:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"user:usr_01h455vb4pex5vy7enb1p677vn","payload":{"user_id":"usr_01h455vb4pex5vy7enb1p677vn","user_search_filter_id":"sf_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn","origin_event_id":"evt_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
    }

    enum FakeResult {
        Result(GenerateSearchFilterMatchNotificationResult),
        StateInvalid,
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
    impl GenerateSearchFilterMatchNotificationUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: GenerateSearchFilterMatchNotificationCommand,
        ) -> Result<
            GenerateSearchFilterMatchNotificationResult,
            GenerateSearchFilterMatchNotificationError,
        > {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = { self.results.lock().expect("results lock").pop_front() };
            match result {
                Some(FakeResult::Result(result)) => Ok(result),
                Some(FakeResult::StateInvalid) => {
                    Err(GenerateSearchFilterMatchNotificationError::ProductListingSourceMismatch)
                }
                Some(FakeResult::Unavailable) => Err(
                    GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed {
                        source: Box::new(sqlx::Error::Tls(Box::new(std::io::Error::other(
                            SENTINEL,
                        )))),
                    },
                ),
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
