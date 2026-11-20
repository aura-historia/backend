//! `product-embedding`: embeds one product listing through Vertex AI behind a source-version guard.

use crate::composition::CompositionError;
use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use embedding::{
    EmbeddingError, EmbeddingGenerator, VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator,
};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{
    Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE, config::required,
    google_application_default_credentials,
};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::{
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
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

const SCOPE: &str = "product-embedding";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";

/// Builds the PostgreSQL-backed, source-version-guarded embedding use case.
///
/// The source read completes before the provider call and the write transaction begins only after
/// it, so no pool connection is occupied while Vertex is awaited.
pub fn compose_product_embedding_use_case(
    pool: PgPool,
    generator: VertexAiEmbeddingGenerator,
) -> Arc<dyn EmbedProductListingEventUseCase> {
    compose_product_embedding_use_case_with_generator(pool, generator)
}

pub fn compose_product_embedding_use_case_with_generator<G>(
    pool: PgPool,
    generator: G,
) -> Arc<dyn EmbedProductListingEventUseCase>
where
    G: EmbeddingGenerator + 'static,
{
    Arc::new(EmbedProductListingEventHandler::new(
        SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
        generator,
        SqlxUnitOfWork::new(pool),
        SqlxProductListingEmbeddingWriterFactory::new(),
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

/// Provider, source, write, and commit failures leave the job retryable. In particular, a
/// provider result does not prove that the guarded PostgreSQL commit completed. Only the failed
/// stage and safe cause facts are kept; the error chain itself is never logged.
fn embedding_error_disposition(
    error: &EmbedProductListingEventError,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventError as E;
    use ProductEmbeddingJobDisposition as D;
    match error {
        E::ServiceOrSystemPrincipalRequired => D::Poison("system_principal_required"),
        E::InvalidInput { .. } => D::Poison("embedding_input_invalid"),
        E::SourceReadFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("source_read", error))
        }
        E::GenerationFailed { .. } => D::DependencyUnavailable(generation_failure(error)),
        E::BeginTransactionFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("write_begin", error))
        }
        E::EmbeddingWriteFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("embedding_write", error))
        }
        E::CommitTransactionFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("write_commit", error))
        }
    }
}

fn generation_failure(error: &EmbedProductListingEventError) -> FailureDiagnostic {
    let failure = find_source::<EmbeddingError>(error);
    FailureDiagnostic::new(
        "generation",
        Dependency::VertexAi,
        failure.map_or(UNCLASSIFIED_FAILURE, EmbeddingError::diagnostic_category),
        failure.and_then(EmbeddingError::http_status),
    )
}

pub fn embedding_disposition(
    outcome: EmbedProductListingEventOutcome,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventOutcome as O;
    match outcome {
        O::Applied => ProductEmbeddingJobDisposition::Complete("applied"),
        O::Duplicate => ProductEmbeddingJobDisposition::Complete("duplicate"),
        O::Stale => ProductEmbeddingJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductEmbeddingJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductEmbeddingJobDisposition::Complete("missing_title"),
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
    let project_id =
        required(VERTEX_AI_PROJECT_ID_ENV).map_err(|e| CompositionError::new(SCOPE, e))?;
    let location = required(VERTEX_AI_LOCATION_ENV).map_err(|e| CompositionError::new(SCOPE, e))?;
    let credentials =
        google_application_default_credentials().map_err(|e| CompositionError::new(SCOPE, e))?;
    let generator = VertexAiEmbeddingGenerator::new(
        VertexAiEmbeddingConfig::new(project_id, location),
        credentials,
    )
    .map_err(|e| CompositionError::new(SCOPE, e))?;
    Ok(Arc::new(ProductEmbeddingJobs::new(
        compose_product_embedding_use_case(pool.clone(), generator),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use product_listing_service::use_cases::EmbedProductListingEventResult;
    use serde_json::{Map, Value, json};
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
    async fn acknowledges_committed_outcomes_and_retries_all_unfinished_records() {
        use EmbedProductListingEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Applied),
            FakeResult::Outcome(O::Duplicate),
            FakeResult::Outcome(O::Stale),
            FakeResult::Outcome(O::IgnoredEvent),
            FakeResult::Outcome(O::MissingTitle),
            FakeResult::Outcome(O::ProductListingNotFound),
            FakeResult::Unavailable,
            FakeResult::PrincipalRejected,
            FakeResult::InputInvalid,
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
                Disposition::Retry(error_class("missing_source")),
                Disposition::Retry(error_class("embedding_unavailable")),
                Disposition::Retry(error_class("system_principal_required")),
                Disposition::Retry(error_class("embedding_input_invalid")),
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
            FakeResult::Outcome(O::Applied),
            FakeResult::Outcome(O::Duplicate),
        ]));
        let handler = ProductEmbeddingJobs::new(use_case);
        let (logs, guard) = CapturedLogs::start();

        handler.handle(&claimed_job(VALID_BODY)).await;
        handler.handle(&claimed_job(VALID_BODY)).await;
        drop(guard);

        let completed = logs.events_where("message", COMPLETED);
        assert_eq!(2, completed.len());
        assert_eq!(Some("applied"), completed[0]["outcome"].as_str());
        assert_eq!(Some(false), completed[0]["guard_rejection"].as_bool());
        assert_eq!(Some("duplicate"), completed[1]["outcome"].as_str());
        assert_eq!(Some(true), completed[1]["guard_rejection"].as_bool());
    }

    #[tokio::test]
    async fn should_distinguish_provider_and_postgres_failures_without_logging_their_causes() {
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::ProviderAuthenticationFailed,
            FakeResult::ProviderThrottled,
            FakeResult::SourceReadTimedOut,
            FakeResult::CommitUnconfirmed,
        ]));
        let handler = ProductEmbeddingJobs::new(use_case);
        let jobs = [
            claimed_job(VALID_BODY),
            claimed_job(VALID_BODY),
            claimed_job(VALID_BODY),
            claimed_job(VALID_BODY),
        ];
        let (logs, guard) = CapturedLogs::start();

        let mut dispositions = Vec::new();
        for job in &jobs {
            dispositions.push(handler.handle(job).await);
        }
        drop(guard);

        assert!(
            dispositions
                .iter()
                .all(|d| *d == Disposition::Retry(error_class("embedding_unavailable")))
        );
        let job_id = |index: usize| jobs[index].id().to_string();
        assert_eq!(
            retained_diagnostics(&logs),
            [
                json!({"job_id": job_id(0), "attempt": 1, "outcome": "embedding_unavailable",
                    "stage": "generation", "dependency": "vertex_ai",
                    "error_category": "authentication"}),
                json!({"job_id": job_id(1), "attempt": 1, "outcome": "embedding_unavailable",
                    "stage": "generation", "dependency": "vertex_ai",
                    "error_category": "http_status", "http_status": 429}),
                json!({"job_id": job_id(2), "attempt": 1, "outcome": "embedding_unavailable",
                    "stage": "source_read", "dependency": "postgres",
                    "error_category": "pool_timed_out"}),
                json!({"job_id": job_id(3), "attempt": 1, "outcome": "embedding_unavailable",
                    "stage": "write_commit", "dependency": "postgres", "error_category": "io"}),
            ]
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    /// The diagnostic fields of each retained-job event; timing varies between runs.
    fn retained_diagnostics(logs: &CapturedLogs) -> Vec<Value> {
        logs.events_where("message", RETAINED)
            .into_iter()
            .map(|fields| {
                Value::Object(
                    fields
                        .into_iter()
                        .filter(|(name, _)| name != "message" && name != "processing_duration_ms")
                        .collect::<Map<_, _>>(),
                )
            })
            .collect()
    }

    const VALID_BODY: &str = r#"{"schema_version":1,"scope":"product-embedding","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#;

    enum FakeResult {
        Outcome(EmbedProductListingEventOutcome),
        Unavailable,
        PrincipalRejected,
        InputInvalid,
        ProviderAuthenticationFailed,
        ProviderThrottled,
        SourceReadTimedOut,
        CommitUnconfirmed,
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
                    Err(EmbedProductListingEventError::GenerationFailed {
                        source: Box::new(std::io::Error::other("unavailable")),
                    })
                }
                Some(FakeResult::PrincipalRejected) => {
                    Err(EmbedProductListingEventError::ServiceOrSystemPrincipalRequired)
                }
                Some(FakeResult::InputInvalid) => {
                    Err(EmbedProductListingEventError::InvalidInput {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    })
                }
                Some(FakeResult::ProviderAuthenticationFailed) => {
                    Err(EmbeddingError::AuthenticationFailed {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    }
                    .into())
                }
                Some(FakeResult::ProviderThrottled) => Err(EmbeddingError::ApiFailure {
                    status: 429_u16.try_into().expect("valid HTTP status"),
                }
                .into()),
                Some(FakeResult::SourceReadTimedOut) => {
                    Err(EmbedProductListingEventError::SourceReadFailed {
                        source: Box::new(sqlx::Error::PoolTimedOut),
                    })
                }
                Some(FakeResult::CommitUnconfirmed) => {
                    Err(EmbedProductListingEventError::CommitTransactionFailed {
                        source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                    })
                }
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
