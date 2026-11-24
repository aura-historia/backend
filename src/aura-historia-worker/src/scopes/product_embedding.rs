//! `product-embedding`: records one product listing's embedding revision as pending embedding
//! work. The [batch rounds](crate::embedding_batches) embed it.

use crate::composition::CompositionError;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::{
    SqlxProductListingEmbeddingQueueFactory, SqlxProductListingEmbeddingSourceReader,
};
use product_listing_service::use_cases::{
    EmbedProductListingCommand, EmbedProductListingEventError, EmbedProductListingEventHandler,
    EmbedProductListingEventOutcome, EmbedProductListingEventUseCase,
};
use sqlx::PgPool;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};

/// Builds the PostgreSQL-backed use case that schedules a revision's embedding.
pub fn compose_product_embedding_use_case(
    pool: PgPool,
) -> Arc<dyn EmbedProductListingEventUseCase> {
    Arc::new(EmbedProductListingEventHandler::new(
        SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
        SqlxUnitOfWork::new(pool),
        SqlxProductListingEmbeddingQueueFactory::new(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl ProductEmbeddingJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "embedding_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }

    /// The guarded write rejected the job as already settled.
    fn is_guard_rejection(self) -> bool {
        matches!(
            self,
            Self::Complete("duplicate" | "stale" | "ignored_event")
        )
    }
}

pub async fn process_product_embedding_job(
    body: &str,
    use_case: &dyn EmbedProductListingEventUseCase,
) -> ProductEmbeddingJobDisposition {
    match decode(body, WorkerScope::ProductListingEmbedding) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductEmbeddingJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &dyn EmbedProductListingEventUseCase,
    job: DomainJob,
) -> ProductEmbeddingJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductEmbeddingJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-embedding:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => embedding_disposition(result.outcome),
        Err(error) => embedding_error_disposition(&error),
    }
}

/// Source, schedule and commit failures retain the job. Only the failed stage and safe cause
/// facts are kept; the error chain itself is never logged.
fn embedding_error_disposition(
    error: &EmbedProductListingEventError,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventError as E;
    use ProductEmbeddingJobDisposition as D;
    let stage = match error {
        E::ServiceOrSystemPrincipalRequired => return D::Poison("system_principal_required"),
        E::SourceReadFailed { .. } => "source_read",
        E::BeginTransactionFailed { .. } => "write_begin",
        E::ScheduleFailed { .. } => "schedule_write",
        E::CommitTransactionFailed { .. } => "write_commit",
    };
    D::DependencyUnavailable(FailureDiagnostic::postgres(stage, error))
}

pub fn embedding_disposition(
    outcome: EmbedProductListingEventOutcome,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventOutcome as O;
    match outcome {
        O::Scheduled => ProductEmbeddingJobDisposition::Complete("scheduled"),
        O::Duplicate => ProductEmbeddingJobDisposition::Complete("duplicate"),
        O::Stale => ProductEmbeddingJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductEmbeddingJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductEmbeddingJobDisposition::Complete("missing_title"),
        O::NotForSale => ProductEmbeddingJobDisposition::Complete("not_for_sale"),
        O::ProductListingNotFound => ProductEmbeddingJobDisposition::Retry("missing_source"),
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<EmbedProductListingCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(EmbedProductListingCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

/// Only completion acknowledges. Every other outcome, including poison, is retried under its
/// category until the queue moves it to custody; the error chain itself is never logged.
pub fn disposition(
    job: &Job,
    outcome: ProductEmbeddingJobDisposition,
    duration: Duration,
) -> Disposition {
    let category = outcome.category();
    if matches!(outcome, ProductEmbeddingJobDisposition::Complete(_)) {
        info!(
            job_id = %job.id(),
            outcome = category,
            guard_rejection = outcome.is_guard_rejection(),
            processing_duration_ms = duration.as_millis(),
            "Product embedding job completed"
        );
        return Disposition::Complete;
    }
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
        processing_duration_ms = duration.as_millis(),
        "Product embedding job retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct ProductEmbeddingJobs {
    use_case: Arc<dyn EmbedProductListingEventUseCase>,
}

impl ProductEmbeddingJobs {
    pub fn new(use_case: Arc<dyn EmbedProductListingEventUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductEmbeddingJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let started_at = Instant::now();
        let outcome = process_product_embedding_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, outcome, started_at.elapsed())
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(ProductEmbeddingJobs::new(
        compose_product_embedding_use_case(pool.clone()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use product_listing_service::use_cases::EmbedProductListingEventResult;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const RETAINED: &str = "Product embedding job retained for retry";
    const COMPLETED: &str = "Product embedding job completed";
    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn acknowledges_committed_and_authoritative_noop_outcomes_but_retries_unfinished_work() {
        use EmbedProductListingEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Scheduled),
            FakeResult::Outcome(O::Duplicate),
            FakeResult::Outcome(O::Stale),
            FakeResult::Outcome(O::IgnoredEvent),
            FakeResult::Outcome(O::MissingTitle),
            FakeResult::Outcome(O::NotForSale),
            FakeResult::Outcome(O::ProductListingNotFound),
            FakeResult::Unavailable,
            FakeResult::PrincipalRejected,
        ]));
        let handler = ProductEmbeddingJobs::new(use_case.clone());

        let mut dispositions = Vec::new();
        for _ in 0..9 {
            dispositions.push(handler.handle(&claimed_job(VALID_BODY)).await);
        }
        dispositions.push(handler.handle(&claimed_job("not-json")).await);

        assert_eq!(
            vec![
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Retry(error_class("missing_source")),
                Disposition::Retry(error_class("embedding_unavailable")),
                Disposition::Retry(error_class("system_principal_required")),
                Disposition::Retry(error_class("invalid_wire_job")),
            ],
            dispositions
        );
        assert_eq!(use_case.calls(), 9);
    }

    #[test]
    fn should_retry_unexpected_payloads_under_their_category() {
        assert_eq!(
            Disposition::Retry(error_class("unexpected_payload")),
            disposition(
                &claimed_job(VALID_BODY),
                ProductEmbeddingJobDisposition::Poison("unexpected_payload"),
                Duration::ZERO,
            )
        );
    }

    #[tokio::test]
    async fn should_mark_guard_rejections_on_completed_jobs() {
        use EmbedProductListingEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Scheduled),
            FakeResult::Outcome(O::Stale),
        ]));
        let handler = ProductEmbeddingJobs::new(use_case);
        let (logs, guard) = CapturedLogs::start();

        handler.handle(&claimed_job(VALID_BODY)).await;
        handler.handle(&claimed_job(VALID_BODY)).await;
        drop(guard);

        let completed = logs.events_where("message", COMPLETED);
        assert_eq!(2, completed.len());
        assert_eq!(Some("scheduled"), completed[0]["outcome"].as_str());
        assert_eq!(Some(false), completed[0]["guard_rejection"].as_bool());
        assert_eq!(Some("stale"), completed[1]["outcome"].as_str());
        assert_eq!(Some(true), completed[1]["guard_rejection"].as_bool());
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let use_case = Arc::new(FakeUseCase::new([FakeResult::ScheduleWriteFailed]));
        let job = claimed_job(VALID_BODY);
        let (logs, guard) = CapturedLogs::start();

        let disposition = ProductEmbeddingJobs::new(use_case).handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("embedding_unavailable")),
            disposition
        );
        let retained = logs.events_where("message", RETAINED);
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("schedule_write"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    const VALID_BODY: &str = r#"{"schema_version":1,"scope":"product-embedding","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#;

    enum FakeResult {
        Outcome(EmbedProductListingEventOutcome),
        Unavailable,
        PrincipalRejected,
        ScheduleWriteFailed,
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
    impl EmbedProductListingEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            _: EmbedProductListingCommand,
        ) -> Result<EmbedProductListingEventResult, EmbedProductListingEventError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = self.results.lock().expect("results lock").pop_front();
            match result {
                Some(FakeResult::Outcome(outcome)) => {
                    Ok(EmbedProductListingEventResult { outcome })
                }
                Some(FakeResult::Unavailable) => {
                    Err(EmbedProductListingEventError::CommitTransactionFailed {
                        source: Box::new(std::io::Error::other("commit result unknown")),
                    })
                }
                Some(FakeResult::PrincipalRejected) => {
                    Err(EmbedProductListingEventError::ServiceOrSystemPrincipalRequired)
                }
                Some(FakeResult::ScheduleWriteFailed) => {
                    Err(EmbedProductListingEventError::ScheduleFailed {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    })
                }
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
