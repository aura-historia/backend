//! `product-listing-opensearch`: projects one committed ProductListing event from PostgreSQL into
//! the live OpenSearch index or into an isolated index generation.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_opensearch::failure::OpenSearchFailure;
use platform_opensearch::transport::{CA_CERTIFICATE_ENV, trust_private_ca};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE, config::required};
use platform_worker::{Job, JobHandler, error_class};
use platform_worker_queue_contract::job_queue::ProjectionGeneration;
use product_listing_opensearch::OpenSearchProductListingSearchProjection;
use product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory;
use product_listing_service::use_cases::{
    ProjectProductListingCommand, ProjectProductListingError, ProjectProductListingHandler,
    ProjectProductListingOutcome, ProjectProductListingUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::warn;
use url::Url;

const SCOPE: &str = "product-listing-opensearch";
const LIVE_INDEX: &str = "product-listings";

const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
const WORKER_STAGE_ENV: &str = "STAGE";

/// Production composition for the one PostgreSQL-to-OpenSearch projection use case.
/// Integration tests call this same root; no test-only handler may diverge from production wiring.
pub fn compose_projection_use_case(
    pool: PgPool,
    open_search: OpenSearch,
    index: impl Into<String>,
) -> Arc<dyn ProjectProductListingUseCase> {
    Arc::new(ProjectProductListingHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        OpenSearchProductListingSearchProjection::with_index(open_search, index),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingOpenSearchJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
    /// Poison caused by persisted state that cannot be read as valid until it is repaired.
    InvalidPersistedState {
        category: &'static str,
        failure: FailureDiagnostic,
    },
}

impl ProductListingOpenSearchJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::Poison(category)
            | Self::InvalidPersistedState { category, .. } => category,
            Self::DependencyUnavailable(_) => "projection_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) | Self::InvalidPersistedState { failure, .. } => {
                Some(failure)
            }
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_product_listing_opensearch_job(
    body: &str,
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
) -> ProductListingOpenSearchJobDisposition {
    match decode(body, WorkerScope::ProductListingOpenSearch) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductListingOpenSearchJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
    job: DomainJob,
) -> ProductListingOpenSearchJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return ProductListingOpenSearchJobDisposition::Poison("unexpected_payload");
    };
    match use_case
        .execute(ProjectProductListingCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => projection_disposition(result.outcome),
        Err(error) => projection_error_disposition(&error),
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn projection_error_disposition(
    error: &ProjectProductListingError,
) -> ProductListingOpenSearchJobDisposition {
    use ProductListingOpenSearchJobDisposition as D;
    use ProjectProductListingError as E;
    match error {
        E::SourceStateInvalid { .. } => D::InvalidPersistedState {
            category: "source_state_invalid",
            failure: FailureDiagnostic::new(
                "source_read",
                Dependency::Postgres,
                INVALID_PERSISTED_STATE,
                None,
            ),
        },
        E::BeginFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("source_begin", error))
        }
        E::SourceReadFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("source_read", error))
        }
        E::CommitFailed { .. } => {
            D::DependencyUnavailable(FailureDiagnostic::postgres("source_commit", error))
        }
        E::WriteFailed { .. } => D::DependencyUnavailable(target_write_failure(error)),
    }
}

/// Without an OpenSearch error in the chain, the adapter refused to build the document from the
/// source (an invalid version or unmappable field), so the cause lies in the source data.
fn target_write_failure(error: &ProjectProductListingError) -> FailureDiagnostic {
    let failure = OpenSearchFailure::find(error);
    FailureDiagnostic::new(
        "target_write",
        Dependency::OpenSearch,
        failure.map_or("document_invalid", |failure| failure.kind()),
        failure.and_then(|failure| failure.http_status()),
    )
}

pub fn projection_disposition(
    outcome: ProjectProductListingOutcome,
) -> ProductListingOpenSearchJobDisposition {
    match outcome {
        ProjectProductListingOutcome::Applied => {
            ProductListingOpenSearchJobDisposition::Complete("applied")
        }
        ProjectProductListingOutcome::Deleted => {
            ProductListingOpenSearchJobDisposition::Complete("deleted")
        }
        ProjectProductListingOutcome::Stale => {
            ProductListingOpenSearchJobDisposition::Complete("stale")
        }
        // Absence of the committed event/source is not evidence that its projection was removed.
        ProjectProductListingOutcome::MissingSource => {
            ProductListingOpenSearchJobDisposition::Retry("missing_source")
        }
    }
}

/// Only completion acknowledges; every other outcome, poison included, is retried. Each retried
/// job is recorded once, keyed by its job ID; the error chain itself is never logged.
pub fn disposition(job: &Job, outcome: &ProductListingOpenSearchJobDisposition) -> Disposition {
    if matches!(outcome, ProductListingOpenSearchJobDisposition::Complete(_)) {
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
        "ProductListing projection job retained for retry or redrive"
    );
    Disposition::Retry(error_class(category))
}

pub struct ProductListingOpenSearchJobs {
    use_case: Arc<dyn ProjectProductListingUseCase>,
}

impl ProductListingOpenSearchJobs {
    pub fn new(use_case: Arc<dyn ProjectProductListingUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductListingOpenSearchJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_product_listing_opensearch_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, &outcome)
    }
}

/// The consumer that projects into the live index.
pub async fn compose_live(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    compose_index(pool, LIVE_INDEX)
}

/// The consumer of one isolated generation; it writes only that generation's index.
pub async fn compose_generation(
    pool: &PgPool,
    generation: &ProjectionGeneration,
) -> Result<Arc<dyn JobHandler>, CompositionError> {
    compose_index(pool, generation.index_name())
}

fn compose_index(
    pool: &PgPool,
    index: impl Into<String>,
) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let open_search = open_search_client(OpenSearchConfig::from_env()?)?;
    Ok(Arc::new(ProductListingOpenSearchJobs::new(
        compose_projection_use_case(pool.clone(), open_search, index),
    )))
}

struct OpenSearchConfig {
    endpoint: Url,
    basic_auth: Option<(String, String)>,
    private_ca: Option<String>,
}

impl OpenSearchConfig {
    fn from_env() -> Result<Self, CompositionError> {
        let config = |name| required(name).map_err(|error| CompositionError::new(SCOPE, error));
        let stage = config(WORKER_STAGE_ENV)?;
        let endpoint = Url::parse(&config(OPENSEARCH_ENDPOINT_URL_ENV)?)
            .map_err(|_| CompositionError::new(SCOPE, "invalid OpenSearch endpoint"))?;
        let basic_auth = if stage == "ephemeral" {
            None
        } else {
            Some((
                config(OPENSEARCH_USERNAME_ENV)?,
                config(OPENSEARCH_PASSWORD_ENV)?,
            ))
        };
        Ok(Self {
            endpoint,
            basic_auth,
            private_ca: std::env::var(CA_CERTIFICATE_ENV).ok(),
        })
    }
}

fn open_search_client(config: OpenSearchConfig) -> Result<OpenSearch, CompositionError> {
    let pool = SingleNodeConnectionPool::new(config.endpoint);
    let builder = trust_private_ca(TransportBuilder::new(pool), config.private_ca.as_deref())
        .map_err(|error| CompositionError::new(SCOPE, error))?;
    let transport = match config.basic_auth {
        Some((username, password)) => builder.auth(Credentials::Basic(username, password)),
        None => builder,
    }
    .build()
    // The builder error can echo the endpoint; only the fact of failure is reported.
    .map_err(|_| CompositionError::new(SCOPE, "failed to configure OpenSearch client"))?;
    Ok(OpenSearch::new(transport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use domain_primitives::event_id::EventId;
    use platform_opensearch::response::OpenSearchStatusError;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_service::use_cases::ProjectProductListingResult;
    use serde_json::{Map, Value, json};
    use sqlx::error::{DatabaseError, ErrorKind};
    use std::{
        borrow::Cow,
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const EVENT_ID: &str = "evt_01h455vb4pex5vy7enb1p677vn";
    const PRODUCT_LISTING_ID: &str = "pl_01h455vb4pex5vy7enb1p677vn";
    const RETAINED: &str = "ProductListing projection job retained for retry or redrive";
    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    async fn handle_each<const N: usize>(
        results: [FakeResult; N],
    ) -> (Vec<Disposition>, Arc<FakeUseCase>) {
        let processor = Arc::new(FakeUseCase::new(results));
        let handler = ProductListingOpenSearchJobs::new(processor.clone());
        let mut dispositions = Vec::with_capacity(N);
        for _ in 0..N {
            dispositions.push(handler.handle(&claimed_job(&valid_body())).await);
        }
        (dispositions, processor)
    }

    #[tokio::test]
    async fn should_acknowledge_completed_duplicate_and_stale_projection_results() {
        let (dispositions, processor) =
            handle_each([FakeResult::Applied, FakeResult::Deleted, FakeResult::Stale]).await;

        assert_eq!(vec![Disposition::Complete; 3], dispositions);
        assert_eq!(processor.calls(), 3);
    }

    #[tokio::test]
    async fn should_preserve_worker_retry_poison_and_dependency_dispositions() {
        let (dispositions, processor) = handle_each([
            FakeResult::Applied,
            FakeResult::Stale,
            FakeResult::MissingSource,
            FakeResult::SourceStateInvalid,
            FakeResult::Unavailable,
        ])
        .await;

        assert_eq!(
            vec![
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Retry(error_class("missing_source")),
                Disposition::Retry(error_class("source_state_invalid")),
                Disposition::Retry(error_class("projection_unavailable")),
            ],
            dispositions
        );
        assert_eq!(processor.calls(), 5);
    }

    #[tokio::test]
    async fn should_retry_deferred_and_malformed_jobs_without_acknowledging_them() {
        let processor = Arc::new(FakeUseCase::new([
            FakeResult::Applied,
            FakeResult::MissingSource,
        ]));
        let handler = ProductListingOpenSearchJobs::new(processor.clone());

        assert_eq!(
            Disposition::Complete,
            handler.handle(&claimed_job(&valid_body())).await
        );
        assert_eq!(
            Disposition::Retry(error_class("missing_source")),
            handler.handle(&claimed_job(&valid_body())).await
        );
        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job("not-json")).await
        );
        assert_eq!(processor.calls(), 2);
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_and_only_safe_cause_facts_once_per_retained_record() {
        let processor = Arc::new(FakeUseCase::new([
            FakeResult::SourceDatabaseFailure,
            FakeResult::TargetForbidden,
            FakeResult::CommitConnectionLost,
            FakeResult::SourceStateInvalid,
        ]));
        let handler = ProductListingOpenSearchJobs::new(processor);
        let jobs = [(); 4].map(|()| claimed_job(&valid_body()));
        let (logs, guard) = CapturedLogs::start();

        for job in &jobs {
            assert!(matches!(handler.handle(job).await, Disposition::Retry(_)));
        }
        drop(guard);

        let id = |index: usize| jobs[index].id().to_string();
        assert_eq!(
            retained_diagnostics(&logs),
            [
                json!({"job_id": id(0), "attempt": 1, "outcome": "projection_unavailable",
                    "stage": "source_read", "dependency": "postgres",
                    "error_category": "database", "sqlstate": "08006"}),
                json!({"job_id": id(1), "attempt": 1, "outcome": "projection_unavailable",
                    "stage": "target_write", "dependency": "opensearch",
                    "error_category": "http_status", "http_status": 403}),
                json!({"job_id": id(2), "attempt": 1, "outcome": "projection_unavailable",
                    "stage": "source_commit", "dependency": "postgres", "error_category": "io"}),
                json!({"job_id": id(3), "attempt": 1, "outcome": "source_state_invalid",
                    "stage": "source_read", "dependency": "postgres",
                    "error_category": "invalid_persisted_state"}),
            ]
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn retained_diagnostics(logs: &CapturedLogs) -> Vec<Value> {
        logs.events_where("message", RETAINED)
            .into_iter()
            .map(|fields| {
                Value::Object(
                    fields
                        .into_iter()
                        .filter(|(name, _)| name != "message")
                        .collect::<Map<_, _>>(),
                )
            })
            .collect()
    }

    fn valid_body() -> String {
        format!(
            r#"{{"schema_version":1,"scope":"product-listing-opensearch","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{EVENT_ID}","ordering_key":"product:{PRODUCT_LISTING_ID}","payload":{{"event_id":"{EVENT_ID}","product_listing_id":"{PRODUCT_LISTING_ID}"}}}}"#,
        )
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Applied,
        Deleted,
        Stale,
        MissingSource,
        SourceStateInvalid,
        Unavailable,
        SourceDatabaseFailure,
        TargetForbidden,
        CommitConnectionLost,
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
    impl ProjectProductListingUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: ProjectProductListingCommand,
        ) -> Result<ProjectProductListingResult, ProjectProductListingError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let next = match self.results.lock() {
                Ok(mut results) => results.pop_front(),
                Err(poisoned) => poisoned.into_inner().pop_front(),
            };
            match next {
                Some(FakeResult::Applied) => Ok(result(ProjectProductListingOutcome::Applied)),
                Some(FakeResult::Deleted) => Ok(result(ProjectProductListingOutcome::Deleted)),
                Some(FakeResult::Stale) => Ok(result(ProjectProductListingOutcome::Stale)),
                Some(FakeResult::MissingSource) => {
                    Ok(result(ProjectProductListingOutcome::MissingSource))
                }
                Some(FakeResult::SourceStateInvalid) => {
                    Err(ProjectProductListingError::SourceStateInvalid {
                        source: Box::new(std::io::Error::other(SENTINEL)),
                    })
                }
                Some(FakeResult::Unavailable) => {
                    Err(ProjectProductListingError::SourceReadFailed {
                        source: Box::new(std::io::Error::other("projection source unavailable")),
                    })
                }
                Some(FakeResult::SourceDatabaseFailure) => {
                    Err(ProjectProductListingError::SourceReadFailed {
                        source: Box::new(sqlx::Error::Database(Box::new(FakeDatabaseError))),
                    })
                }
                Some(FakeResult::TargetForbidden) => Err(ProjectProductListingError::WriteFailed {
                    source: Box::new(OpenSearchStatusError {
                        status: opensearch::http::StatusCode::FORBIDDEN,
                        body: SENTINEL.to_owned(),
                    }),
                }),
                Some(FakeResult::CommitConnectionLost) => {
                    Err(ProjectProductListingError::CommitFailed {
                        source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                    })
                }
                None => panic!("missing fake projection result"),
            }
        }
    }

    /// A PostgreSQL error whose server message repeats a persisted value.
    #[derive(Debug)]
    struct FakeDatabaseError;

    impl std::fmt::Display for FakeDatabaseError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "connection failure for {SENTINEL}")
        }
    }

    impl std::error::Error for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &str {
            SENTINEL
        }

        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed("08006"))
        }

        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }

        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }

        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn result(outcome: ProjectProductListingOutcome) -> ProjectProductListingResult {
        ProjectProductListingResult { outcome }
    }

    #[test]
    fn should_keep_wire_fixture_ids_canonical() {
        assert!(EVENT_ID.parse::<EventId>().is_ok());
        assert!(PRODUCT_LISTING_ID.parse::<ProductListingId>().is_ok());
    }
}
