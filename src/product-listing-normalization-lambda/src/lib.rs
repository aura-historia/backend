use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::{
    Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE, LambdaInvocationBudget,
};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxNormalizationDiagnosticCaseWriterFactory, SqlxPendingProductListingRawStreamReader,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionMode,
    NormalizeProductListingRawRevisionUseCase,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);
// Bound each attempt; capped wake-ups remain failed SQS items until the stream drains.
pub const MAX_REVISIONS_PER_STREAM: u32 = 32;
pub const PENDING_STREAM_LIMIT: u32 = 100;

/// Builds the one PostgreSQL-backed normalization use case used by production and integration tests.
pub fn compose_normalization_use_case(
    pool: sqlx::PgPool,
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
            SqlxPendingProductListingRawStreamReader::new(pool),
        )
        .with_diagnostic_cases(SqlxNormalizationDiagnosticCaseWriterFactory::new())
        .with_crawler_shipping(Arc::new(shipping)),
    )
}

/// Uses a single deadline for credential refresh, composition, record work, and Lambda response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn NormalizeProductListingRawRevisionUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn NormalizeProductListingRawRevisionUseCase + Send + Sync),
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
    use_case: &(dyn NormalizeProductListingRawRevisionUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_product_listing_raw_normalization_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category, failure) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(
                    disposition,
                    ProductListingRawNormalizationJobDisposition::Complete(_)
                ),
                disposition.category(),
                disposition.failure_diagnostic(),
            ),
            RecordOutcome::MissingBody => (false, "missing_message_body", None),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget", None),
            RecordOutcome::TimedOut => (false, "execution_timeout", None),
            RecordOutcome::Panicked => (false, "handler_panicked", None),
        };
        if !complete {
            warn!(
                message_id = %attempt.message_id,
                outcome = category,
                stage = failure.map(FailureDiagnostic::stage),
                dependency = failure.and_then(FailureDiagnostic::dependency),
                error_category = failure.map(FailureDiagnostic::category),
                sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
                "Raw normalization record retained for SQS retry or redrive"
            );
        }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished ProductListing raw normalization batch"
    );
    Ok(response)
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
        | E::NormalizationConfigurationFailed { .. } => {
            ProductListingRawNormalizationJobDisposition::StateInvalid(failure)
        }
        E::PendingStreamReadFailed { .. }
        | E::BeginTransactionFailed { .. }
        | E::PersistenceFailed { .. }
        | E::CanonicalWriteFailed { .. }
        | E::CommitTransactionFailed { .. }
        | E::DiagnosticCaseRecordFailed { .. } => {
            ProductListingRawNormalizationJobDisposition::DependencyUnavailable(failure)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::TransactionError;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::ports::ProductListingRawStreamId;
    use product_service::ports::ProductListingRawNormalizationPortError;
    use product_service::use_cases::{
        NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
        NormalizeProductListingRawRevisionResult, ProductListingRawNormalizationStreamFailure,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use test_api::CapturedLogs;

    #[tokio::test]
    async fn acknowledges_only_a_fully_drained_or_durably_rejected_stream() {
        let use_case = FakeUseCase::new([
            FakeResult::Drained,
            FakeResult::Rejected,
            FakeResult::Capped,
            FakeResult::Drained,
            FakeResult::FailedStream,
            FakeResult::Failure,
        ]);
        let response = handler(
            events([
                (Some("drained"), valid_body()),
                (Some("rejected"), valid_body()),
                (Some("capped"), valid_body()),
                (Some("completed-after-capped"), valid_body()),
                (Some("failed-stream"), valid_body()),
                (Some("failure"), valid_body()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(
            failure_ids(response),
            ["capped", "failed-stream", "failure"]
        );
        assert_eq!(use_case.calls(), 6);
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_once_per_retained_record_without_its_cause() {
        let use_case = FakeUseCase::new([
            FakeResult::FailedStream,
            FakeResult::CommitLost,
            FakeResult::StateInvalid,
        ]);
        let (logs, guard) = CapturedLogs::start();

        let response = handler(
            events([
                (Some("failed-stream"), valid_body()),
                (Some("commit"), valid_body()),
                (Some("state-invalid"), valid_body()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");
        drop(guard);

        assert_eq!(
            failure_ids(response),
            ["failed-stream", "commit", "state-invalid"]
        );
        let field = |fields: &serde_json::Map<String, serde_json::Value>, name: &str| {
            fields
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        let diagnostic = |fields: &serde_json::Map<String, serde_json::Value>| {
            ["outcome", "stage", "dependency", "error_category"].map(|name| field(fields, name))
        };
        let retained = logs
            .events_where(
                "message",
                "Raw normalization record retained for SQS retry or redrive",
            )
            .iter()
            .map(diagnostic)
            .collect::<Vec<_>>();
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
        assert_eq!(expected.as_slice(), retained.as_slice());
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
    async fn retains_malformed_timeout_and_panic_records() {
        for result in [FakeResult::Pending, FakeResult::Panic] {
            let use_case = FakeUseCase::new([result]);
            let response = handler_with_budget(
                events([(Some("retry"), valid_body())]),
                &use_case,
                Duration::from_millis(1),
                Duration::from_millis(1),
            )
            .await
            .expect("handler response");
            assert_eq!(failure_ids(response), ["retry"]);
        }

        let use_case = FakeUseCase::new([FakeResult::Drained]);
        let response = handler(events([(Some("poison"), "not-json".to_owned())]), &use_case)
            .await
            .expect("handler response");
        assert_eq!(failure_ids(response), ["poison"]);
        assert_eq!(use_case.calls(), 0);
    }

    #[tokio::test]
    async fn retains_unstarted_records_after_the_shared_invocation_budget_is_spent() {
        let use_case = FakeUseCase::new([FakeResult::Pending, FakeResult::Drained]);
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
    fn retains_every_record_when_setup_cannot_complete() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial batch response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"product-listing-normalization","job_type":"PRODUCT_LISTING_RAW_REVISION","idempotency_key":"product-listing-raw-revision:prr_01j0000000e008000000000002","ordering_key":"product-listing-raw-stream:prs_01j0000000e008000000000001","payload":{"product_listing_raw_stream_id":"prs_01j0000000e008000000000001","product_listing_raw_revision_id":"prr_01j0000000e008000000000002","revision":1}}"#.to_owned()
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
        Drained,
        Rejected,
        Capped,
        FailedStream,
        Failure,
        CommitLost,
        StateInvalid,
        Pending,
        Panic,
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
                Some(FakeResult::Pending) => std::future::pending().await,
                Some(FakeResult::Panic) => panic!("test panic"),
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
