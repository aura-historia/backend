//! `product-listing-normalization`: drains one raw ProductListing stream from its persisted
//! progress into the canonical listing.

use crate::composition::CompositionError;
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE};
use platform_worker::{Job, JobHandler, error_class};
use product_listing_postgres::{
    SqlxDescriptionBoilerplateStore, SqlxNormalizationDiagnosticCaseWriterFactory,
    SqlxPendingProductListingRawStreamReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawNormalizationWriterFactory, SqlxProductListingRepositoryFactory,
};
use product_service::ports::DescriptionBoilerplatePortError;
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionMode,
    NormalizeProductListingRawRevisionUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;
use tracing::{info, warn};

// Bound each attempt; a capped wake-up is retried until its stream drains.
pub const MAX_REVISIONS_PER_STREAM: u32 = 32;
pub const PENDING_STREAM_LIMIT: u32 = 100;

/// Builds the one PostgreSQL-backed normalization use case used by production and integration tests.
pub fn compose_normalization_use_case(
    pool: PgPool,
) -> Arc<dyn NormalizeProductListingRawRevisionUseCase> {
    let shipping = product_listing_service::crawler_shipping::ApplyCrawlerShippingHandler::new(
        product_listing_postgres::delivery::SqlxCrawlerShippingSourceReader::new(pool.clone()),
        product_listing_service::use_cases::ChangeProductListingDeliveryHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            product_listing_postgres::delivery::SqlxProductListingDeliveryReferencesFactory,
        ),
    );
    Arc::new(
        NormalizeProductListingRawRevisionHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRawNormalizationWriterFactory::new(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPendingProductListingRawStreamReader::new(pool.clone()),
        )
        .with_diagnostic_cases(SqlxNormalizationDiagnosticCaseWriterFactory::new())
        .with_crawler_shipping(Arc::new(shipping))
        .with_description_boilerplate(Arc::new(SqlxDescriptionBoilerplateStore::new(pool))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingRawNormalizationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    /// Reconciliation streams failed; each failure is recorded with its stream ID.
    StreamFailed,
    Poison(&'static str),
    /// Poison caused by stored state or configuration that cannot normalize until repaired.
    StateInvalid(FailureDiagnostic),
}

impl ProductListingRawNormalizationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "normalization_unavailable",
            Self::StreamFailed => "normalization_stream_failed",
            Self::StateInvalid(_) => "normalization_state_invalid",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) | Self::StateInvalid(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) | Self::StreamFailed => None,
        }
    }
}

/// Decode and execute one compact schema-1 raw-normalization wake-up without exposing transport
/// DTOs to the service. Stream ordering and progress remain PostgreSQL-owned.
pub async fn process_product_listing_raw_normalization_job(
    body: &str,
    use_case: &(dyn NormalizeProductListingRawRevisionUseCase + Send + Sync),
) -> ProductListingRawNormalizationJobDisposition {
    let job = match decode(body, WorkerScope::ProductListingRawNormalization) {
        Ok(job) => job,
        Err(_) => return ProductListingRawNormalizationJobDisposition::Poison("invalid_wire_job"),
    };
    let command = match command_from_job(job) {
        Ok(command) => command,
        Err(_) => {
            return ProductListingRawNormalizationJobDisposition::Poison("unexpected_payload");
        }
    };
    process_product_listing_raw_normalization_job_for_command(use_case, command).await
}

pub async fn process_product_listing_raw_normalization_job_for_command(
    use_case: &(dyn NormalizeProductListingRawRevisionUseCase + Send + Sync),
    command: NormalizeProductListingRawRevisionCommand,
) -> ProductListingRawNormalizationJobDisposition {
    match use_case.execute(command).await {
        Ok(result) => {
            info!(
                processed_revisions = result.revisions.len(),
                normalization_failures = result.stream_failures.len(),
                "raw stream drain finished"
            );
            for stream in &result.stream_failures {
                let failure = normalization_failure(&stream.error);
                warn!(
                    product_listing_raw_stream_id = %stream.product_listing_raw_stream_id,
                    stage = failure.stage(),
                    dependency = failure.dependency(),
                    error_category = failure.category(),
                    sqlstate = failure.sqlstate(),
                    "Raw normalization stream failed"
                );
            }
            if !result.stream_failures.is_empty() {
                ProductListingRawNormalizationJobDisposition::StreamFailed
            } else if !result.continuation_stream_ids.is_empty() {
                ProductListingRawNormalizationJobDisposition::Retry("normalization_continuation")
            } else {
                ProductListingRawNormalizationJobDisposition::Complete("stream_drained")
            }
        }
        Err(error) => normalization_error_disposition(&error),
    }
}

fn normalization_error_disposition(
    error: &NormalizeProductListingRawRevisionError,
) -> ProductListingRawNormalizationJobDisposition {
    use NormalizeProductListingRawRevisionError as E;
    let failure = normalization_failure(error);
    match error {
        E::InvalidLimit
        | E::InvalidPersistedState { .. }
        | E::UnsupportedStoredSchemaVersion
        | E::NormalizationConfigurationFailed { .. }
        | E::DescriptionBoilerplateReadFailed {
            source: DescriptionBoilerplatePortError::InvalidPersistedState { .. },
        } => ProductListingRawNormalizationJobDisposition::StateInvalid(failure),
        E::PendingStreamReadFailed { .. }
        | E::BeginTransactionFailed { .. }
        | E::PersistenceFailed { .. }
        | E::CanonicalWriteFailed { .. }
        | E::CommitTransactionFailed { .. }
        | E::DiagnosticCaseRecordFailed { .. }
        | E::DescriptionBoilerplateReadFailed {
            source: DescriptionBoilerplatePortError::Persistence { .. },
        } => ProductListingRawNormalizationJobDisposition::DependencyUnavailable(failure),
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn normalization_failure(error: &NormalizeProductListingRawRevisionError) -> FailureDiagnostic {
    use NormalizeProductListingRawRevisionError as E;
    match error {
        E::InvalidLimit => FailureDiagnostic::rejected("normalization", "invalid_limit"),
        E::NormalizationConfigurationFailed { .. } => {
            FailureDiagnostic::rejected("normalization", "configuration_invalid")
        }
        E::InvalidPersistedState { .. } => FailureDiagnostic::new(
            "normalization_persistence",
            Dependency::Postgres,
            INVALID_PERSISTED_STATE,
            None,
        ),
        E::UnsupportedStoredSchemaVersion => FailureDiagnostic::new(
            "raw_revision_read",
            Dependency::Postgres,
            "schema_version_unsupported",
            None,
        ),
        E::PendingStreamReadFailed { .. } => {
            FailureDiagnostic::postgres("pending_stream_read", error)
        }
        E::BeginTransactionFailed { .. } => {
            FailureDiagnostic::postgres("normalization_begin", error)
        }
        E::PersistenceFailed { .. } => {
            FailureDiagnostic::postgres("normalization_persistence", error)
        }
        E::CanonicalWriteFailed { .. } => FailureDiagnostic::postgres("canonical_write", error),
        E::CommitTransactionFailed { .. } => {
            FailureDiagnostic::postgres("normalization_commit", error)
        }
        E::DiagnosticCaseRecordFailed { .. } => {
            FailureDiagnostic::postgres("diagnostic_case_record", error)
        }
        E::DescriptionBoilerplateReadFailed {
            source: DescriptionBoilerplatePortError::InvalidPersistedState { .. },
        } => FailureDiagnostic::new(
            "description_boilerplate_read",
            Dependency::Postgres,
            INVALID_PERSISTED_STATE,
            None,
        ),
        E::DescriptionBoilerplateReadFailed { .. } => {
            FailureDiagnostic::postgres("description_boilerplate_read", error)
        }
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<NormalizeProductListingRawRevisionCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingRawRevision(revision) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::RawRevision {
            product_listing_raw_stream_id: revision.product_listing_raw_stream_id,
            product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
            revision: revision.revision,
        },
        max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
        pending_stream_limit: PENDING_STREAM_LIMIT,
    })
}

/// Only a drained stream or a durable rejection acknowledges; every other outcome, poison
/// included, is retried. Each retried job is recorded once, keyed by its job ID; the error chain
/// itself is never logged.
pub fn disposition(
    job: &Job,
    outcome: &ProductListingRawNormalizationJobDisposition,
) -> Disposition {
    if matches!(
        outcome,
        ProductListingRawNormalizationJobDisposition::Complete(_)
    ) {
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
        sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
        "Raw normalization job retained for retry or redrive"
    );
    Disposition::Retry(error_class(category))
}

pub struct ProductListingNormalizationJobs {
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
}

impl ProductListingNormalizationJobs {
    pub fn new(use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductListingNormalizationJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let outcome =
            process_product_listing_raw_normalization_job(job.payload(), self.use_case.as_ref())
                .await;
        disposition(job, &outcome)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(ProductListingNormalizationJobs::new(
        compose_normalization_use_case(pool.clone()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use application::transaction::TransactionError;
    use product_listing_service::ports::ProductListingRawStreamId;
    use product_service::ports::ProductListingRawNormalizationPortError;
    use product_service::use_cases::{
        NormalizeProductListingRawRevisionResult, ProductListingRawNormalizationStreamFailure,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const RETAINED: &str = "Raw normalization job retained for retry or redrive";

    #[test]
    fn should_classify_boilerplate_read_failures_by_their_cause() {
        let read_failed = |source| {
            normalization_error_disposition(
                &NormalizeProductListingRawRevisionError::DescriptionBoilerplateReadFailed {
                    source,
                },
            )
        };
        let cause = || application::error::box_error(std::io::Error::other("boilerplate"));

        let invalid =
            read_failed(DescriptionBoilerplatePortError::InvalidPersistedState { source: cause() });
        let unavailable =
            read_failed(DescriptionBoilerplatePortError::Persistence { source: cause() });

        assert!(matches!(
            invalid,
            ProductListingRawNormalizationJobDisposition::StateInvalid(failure)
                if failure.stage() == "description_boilerplate_read"
                    && failure.category() == INVALID_PERSISTED_STATE
        ));
        assert!(matches!(
            unavailable,
            ProductListingRawNormalizationJobDisposition::DependencyUnavailable(failure)
                if failure.stage() == "description_boilerplate_read"
                    && failure.dependency() == Some("postgres")
        ));
    }

    #[tokio::test]
    async fn acknowledges_only_a_fully_drained_or_durably_rejected_stream() {
        let expected = [
            (FakeResult::Drained, Disposition::Complete),
            (FakeResult::Rejected, Disposition::Complete),
            (
                FakeResult::Capped,
                Disposition::Retry(error_class("normalization_continuation")),
            ),
            (FakeResult::Drained, Disposition::Complete),
            (
                FakeResult::FailedStream,
                Disposition::Retry(error_class("normalization_stream_failed")),
            ),
            (
                FakeResult::Failure,
                Disposition::Retry(error_class("normalization_state_invalid")),
            ),
            (
                FakeResult::CommitLost,
                Disposition::Retry(error_class("normalization_unavailable")),
            ),
            (
                FakeResult::StateInvalid,
                Disposition::Retry(error_class("normalization_state_invalid")),
            ),
        ];
        let use_case = Arc::new(FakeUseCase::new(
            expected.each_ref().map(|(result, _)| *result),
        ));
        let handler = ProductListingNormalizationJobs::new(use_case.clone());

        for (_, disposition) in &expected {
            assert_eq!(
                *disposition,
                handler.handle(&claimed_job(&valid_body())).await
            );
        }
        assert_eq!(use_case.calls(), expected.len());
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_once_per_retained_record_without_its_cause() {
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::FailedStream,
            FakeResult::CommitLost,
            FakeResult::StateInvalid,
        ]));
        let handler = ProductListingNormalizationJobs::new(use_case);
        let jobs = [(); 3].map(|()| claimed_job(&valid_body()));
        let (logs, guard) = CapturedLogs::start();

        for job in &jobs {
            assert!(matches!(handler.handle(job).await, Disposition::Retry(_)));
        }
        drop(guard);

        let field = |fields: &serde_json::Map<String, serde_json::Value>, name: &str| {
            fields
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        let diagnostic = |fields: &serde_json::Map<String, serde_json::Value>| {
            ["outcome", "stage", "dependency", "error_category"].map(|name| field(fields, name))
        };
        let retained = logs.events_where("message", RETAINED);
        assert_eq!(
            jobs.iter()
                .map(|job| Some(job.id().to_string()))
                .collect::<Vec<_>>(),
            retained
                .iter()
                .map(|fields| field(fields, "job_id"))
                .collect::<Vec<_>>()
        );
        let expected = [
            [Some("normalization_stream_failed"), None, None, None],
            [
                Some("normalization_unavailable"),
                Some("normalization_commit"),
                Some("postgres"),
                Some("io"),
            ],
            [
                Some("normalization_state_invalid"),
                Some("normalization_persistence"),
                Some("postgres"),
                Some("invalid_persisted_state"),
            ],
        ]
        .map(|fields| fields.map(|value| value.map(str::to_owned)));
        assert_eq!(
            expected.as_slice(),
            retained
                .iter()
                .map(diagnostic)
                .collect::<Vec<_>>()
                .as_slice()
        );
        let streams = logs.events_where("message", "Raw normalization stream failed");
        assert_eq!(1, streams.len());
        assert_eq!(
            Some(stream_id().to_string()),
            field(&streams[0], "product_listing_raw_stream_id")
        );
        assert_eq!(
            [
                None,
                Some("normalization_persistence"),
                Some("postgres"),
                Some("pool_timed_out")
            ]
            .map(|value| value.map(str::to_owned)),
            diagnostic(&streams[0])
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn should_retry_without_invoking_the_use_case_when_the_job_is_malformed() {
        let use_case = Arc::new(FakeUseCase::new([FakeResult::Drained]));
        let handler = ProductListingNormalizationJobs::new(use_case.clone());

        assert_eq!(
            Disposition::Retry(error_class("invalid_wire_job")),
            handler.handle(&claimed_job("not-json")).await
        );
        assert_eq!(use_case.calls(), 0);
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"product-listing-normalization","job_type":"PRODUCT_LISTING_RAW_REVISION","idempotency_key":"product-listing-raw-revision:prr_01j0000000e008000000000002","ordering_key":"product-listing-raw-stream:prs_01j0000000e008000000000001","payload":{"product_listing_raw_stream_id":"prs_01j0000000e008000000000001","product_listing_raw_revision_id":"prr_01j0000000e008000000000002","revision":1}}"#.to_owned()
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Drained,
        Rejected,
        Capped,
        FailedStream,
        Failure,
        CommitLost,
        StateInvalid,
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
    impl NormalizeProductListingRawRevisionUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: NormalizeProductListingRawRevisionCommand,
        ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
        {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let next = self.results.lock().expect("results lock").pop_front();
            match next {
                Some(FakeResult::Drained) | Some(FakeResult::Rejected) => Ok(Default::default()),
                Some(FakeResult::Capped) => Ok(NormalizeProductListingRawRevisionResult {
                    continuation_stream_ids: vec![stream_id()],
                    ..Default::default()
                }),
                Some(FakeResult::FailedStream) => Ok(NormalizeProductListingRawRevisionResult {
                    stream_failures: vec![ProductListingRawNormalizationStreamFailure {
                        product_listing_raw_stream_id: stream_id(),
                        error: NormalizeProductListingRawRevisionError::PersistenceFailed {
                            source: ProductListingRawNormalizationPortError::Persistence {
                                source: Box::new(sqlx::Error::PoolTimedOut),
                            },
                        },
                    }],
                    ..Default::default()
                }),
                Some(FakeResult::Failure) => {
                    Err(NormalizeProductListingRawRevisionError::InvalidLimit)
                }
                Some(FakeResult::CommitLost) => Err(
                    NormalizeProductListingRawRevisionError::CommitTransactionFailed {
                        source: TransactionError::CommitFailed(Box::new(sqlx::Error::Io(
                            std::io::Error::other("SENTINEL_PAYLOAD_TOKEN_person@example.com"),
                        ))),
                    },
                ),
                Some(FakeResult::StateInvalid) => Err(
                    NormalizeProductListingRawRevisionError::InvalidPersistedState {
                        source: ProductListingRawNormalizationPortError::InvalidPersistedState {
                            source: application::error::static_error(
                                "SENTINEL_PAYLOAD_TOKEN_person@example.com",
                            ),
                        },
                    },
                ),
                None => Ok(Default::default()),
            }
        }
    }

    fn stream_id() -> ProductListingRawStreamId {
        ProductListingRawStreamId::try_from(uuid::Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0001,
        ))
        .expect("valid UUIDv7 raw stream ID")
    }
}
