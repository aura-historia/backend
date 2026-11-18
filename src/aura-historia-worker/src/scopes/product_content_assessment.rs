//! `product-content-assessment`: assesses one product listing's content from PostgreSQL alone.

use crate::composition::CompositionError;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSourceReader,
    SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::use_cases::{
    AssessProductListingContentCommand, AssessProductListingContentEventError,
    AssessProductListingContentEventHandler, AssessProductListingContentEventOutcome,
    AssessProductListingContentEventUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::warn;

/// Builds the PostgreSQL-only assessment use case.
pub fn compose_product_content_assessment_use_case(
    pool: PgPool,
) -> Arc<dyn AssessProductListingContentEventUseCase> {
    Arc::new(AssessProductListingContentEventHandler::new(
        SqlxProductListingContentAssessmentSourceReader::new(pool.clone()),
        SqlxUnitOfWork::new(pool),
        SqlxProductListingContentAssessmentWriterFactory::new(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductContentAssessmentJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl ProductContentAssessmentJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "assessment_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_product_content_assessment_job(
    body: &str,
    use_case: &dyn AssessProductListingContentEventUseCase,
) -> ProductContentAssessmentJobDisposition {
    match decode(body, WorkerScope::ProductListingContentAssessment) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductContentAssessmentJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &dyn AssessProductListingContentEventUseCase,
    job: DomainJob,
) -> ProductContentAssessmentJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductContentAssessmentJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-content-assessment:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => assessment_disposition(result.outcome),
        Err(error) => assessment_error_disposition(&error),
    }
}

/// A provider is intentionally not part of this scope. Source, write, commit, and unknown
/// completion errors must remain retryable rather than being acknowledged. Only the failed stage
/// and safe cause facts are kept; the error chain itself is never logged.
fn assessment_error_disposition(
    error: &AssessProductListingContentEventError,
) -> ProductContentAssessmentJobDisposition {
    use AssessProductListingContentEventError as E;
    use ProductContentAssessmentJobDisposition as D;
    let stage = match error {
        E::ServiceOrSystemPrincipalRequired => return D::Poison("system_principal_required"),
        E::SourceReadFailed { .. } => "source_read",
        E::BeginTransactionFailed { .. } => "write_begin",
        E::AssessmentWriteFailed { .. } => "assessment_write",
        E::CommitTransactionFailed { .. } => "write_commit",
    };
    D::DependencyUnavailable(FailureDiagnostic::postgres(stage, error))
}

pub fn assessment_disposition(
    outcome: AssessProductListingContentEventOutcome,
) -> ProductContentAssessmentJobDisposition {
    use AssessProductListingContentEventOutcome as O;
    match outcome {
        O::Applied => ProductContentAssessmentJobDisposition::Complete("applied"),
        O::Cleared => ProductContentAssessmentJobDisposition::Complete("cleared"),
        O::Duplicate => ProductContentAssessmentJobDisposition::Complete("duplicate"),
        O::Stale => ProductContentAssessmentJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductContentAssessmentJobDisposition::Complete("ignored_event"),
        O::ProductListingNotFound => {
            ProductContentAssessmentJobDisposition::Retry("missing_source")
        }
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<AssessProductListingContentCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(AssessProductListingContentCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

/// Only completion acknowledges. Every other outcome, including poison, is retried under its
/// category until the queue moves it to custody; the error chain itself is never logged.
pub fn disposition(job: &Job, outcome: ProductContentAssessmentJobDisposition) -> Disposition {
    if matches!(outcome, ProductContentAssessmentJobDisposition::Complete(_)) {
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
        "Product content-assessment job retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct ProductContentAssessmentJobs {
    use_case: Arc<dyn AssessProductListingContentEventUseCase>,
}

impl ProductContentAssessmentJobs {
    pub fn new(use_case: Arc<dyn AssessProductListingContentEventUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductContentAssessmentJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_product_content_assessment_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(ProductContentAssessmentJobs::new(
        compose_product_content_assessment_use_case(pool.clone()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use product_listing_service::use_cases::AssessProductListingContentEventResult;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const RETAINED: &str = "Product content-assessment job retained for retry";
    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn acknowledges_committed_guarded_outcomes_and_retries_all_unfinished_records() {
        use AssessProductListingContentEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Applied),
            FakeResult::Outcome(O::Cleared),
            FakeResult::Outcome(O::Duplicate),
            FakeResult::Outcome(O::Stale),
            FakeResult::Outcome(O::IgnoredEvent),
            FakeResult::Outcome(O::ProductListingNotFound),
            FakeResult::Unavailable,
            FakeResult::PrincipalRejected,
        ]));
        let handler = ProductContentAssessmentJobs::new(use_case.clone());

        let mut dispositions = Vec::new();
        for _ in 0..8 {
            dispositions.push(handler.handle(&claimed_job(VALID_BODY)).await);
        }
        dispositions.push(handler.handle(&claimed_job("not-json")).await);
        dispositions.push(
            handler
                .handle(&claimed_job(
                    &VALID_BODY.replace("product-content-assessment", "product-embedding"),
                ))
                .await,
        );

        assert_eq!(
            vec![
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Retry(error_class("missing_source")),
                Disposition::Retry(error_class("assessment_unavailable")),
                Disposition::Retry(error_class("system_principal_required")),
                Disposition::Retry(error_class("invalid_wire_job")),
                Disposition::Retry(error_class("invalid_wire_job")),
            ],
            dispositions
        );
        assert_eq!(use_case.calls(), 8);
    }

    #[test]
    fn should_retry_poison_and_unexpected_payloads_under_their_category() {
        let job = claimed_job(VALID_BODY);
        for (outcome, expected) in [
            (
                ProductContentAssessmentJobDisposition::Poison("unexpected_payload"),
                "unexpected_payload",
            ),
            (
                ProductContentAssessmentJobDisposition::Retry("missing_source"),
                "missing_source",
            ),
        ] {
            assert_eq!(
                Disposition::Retry(error_class(expected)),
                disposition(&job, outcome)
            );
        }
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let use_case = Arc::new(FakeUseCase::new([FakeResult::Unavailable]));
        let job = claimed_job(VALID_BODY);
        let (logs, guard) = CapturedLogs::start();

        let disposition = ProductContentAssessmentJobs::new(use_case)
            .handle(&job)
            .await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("assessment_unavailable")),
            disposition
        );
        let retained = logs.events_where("message", RETAINED);
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(
            Some("assessment_unavailable"),
            retained[0]["outcome"].as_str()
        );
        assert_eq!(Some("source_read"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("io"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    const VALID_BODY: &str = r#"{"schema_version":1,"scope":"product-content-assessment","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#;

    enum FakeResult {
        Outcome(AssessProductListingContentEventOutcome),
        Unavailable,
        PrincipalRejected,
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
    impl AssessProductListingContentEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _context: &OperationContext,
            _command: AssessProductListingContentCommand,
        ) -> Result<AssessProductListingContentEventResult, AssessProductListingContentEventError>
        {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = { self.results.lock().expect("results lock").pop_front() };
            match result {
                Some(FakeResult::Outcome(outcome)) => {
                    Ok(AssessProductListingContentEventResult { outcome })
                }
                Some(FakeResult::Unavailable) => {
                    Err(AssessProductListingContentEventError::SourceReadFailed {
                        source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                    })
                }
                Some(FakeResult::PrincipalRejected) => {
                    Err(AssessProductListingContentEventError::ServiceOrSystemPrincipalRequired)
                }
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
