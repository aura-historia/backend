//! `product-listing-ingestion`: applies one admitted ProductListing ingestion command in its
//! listing's FIFO group.

use crate::composition::CompositionError;
use async_trait::async_trait;
use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::FailureDiagnostic;
use platform_worker::{Job, JobHandler, error_class};
use product_listing_ingestion_queue::codec;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingCommandReceiptStoreFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    ProcessProductListingIngestionHandler, ProcessProductListingIngestionUseCase,
    ProductListingIngestionCompletion, ProductListingIngestionFailureClass,
};
use sqlx::PgPool;
use std::{sync::Arc, time::Instant};
use tracing::{info, warn};

/// Compose adapters only; the service use case owns the transaction and command receipt.
pub fn compose_ingestion_use_case(pool: PgPool) -> Arc<dyn ProcessProductListingIngestionUseCase> {
    Arc::new(
        ProcessProductListingIngestionHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
            SqlxAuctionReferenceValidatorFactory::new(),
            SqlxProductListingRawCaptureWriterFactory::new(),
            SqlxProductListingCommandReceiptStoreFactory::new(),
        )
        .with_delivery_references(
            product_listing_postgres::delivery::SqlxProductListingDeliveryReferencesFactory,
        ),
    )
}

pub struct ProductListingIngestionJobs {
    use_case: Arc<dyn ProcessProductListingIngestionUseCase>,
}

impl ProductListingIngestionJobs {
    pub fn new(use_case: Arc<dyn ProcessProductListingIngestionUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductListingIngestionJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let started = Instant::now();
        let (command, outcome) = process_ingestion_job(job.payload(), self.use_case.as_ref()).await;
        let (disposition, category, failure) = match &outcome {
            JobDisposition::Complete(category) => (AttemptDisposition::Completed, *category, None),
            JobDisposition::Rejected(category) => (AttemptDisposition::Rejected, *category, None),
            JobDisposition::Unprocessable(category) => {
                (AttemptDisposition::Unprocessable, *category, None)
            }
            JobDisposition::Failed(failure) => {
                (AttemptDisposition::Retained, failure.stage(), Some(failure))
            }
            JobDisposition::Contended(category) => (AttemptDisposition::Contended, *category, None),
        };
        log_attempt(
            disposition,
            category,
            failure,
            command.as_ref(),
            job,
            started.elapsed().as_millis(),
        );
        disposition.queue_disposition(category)
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    Ok(Arc::new(ProductListingIngestionJobs::new(
        compose_ingestion_use_case(pool.clone()),
    )))
}

/// How one attempted job leaves the queue.
#[derive(Clone, Copy)]
enum AttemptDisposition {
    Completed,
    /// A deterministic business rejection: acknowledged without a receipt so it cannot hold
    /// its FIFO group. The producer must resubmit corrected input.
    Rejected,
    /// An invalid job, identity conflict or invalid persisted state: parked by the queue on
    /// redelivery so later commands of its group can continue.
    Unprocessable,
    Retained,
    /// Retained because a concurrent writer won a race; no dependency failed.
    Contended,
}

impl AttemptDisposition {
    fn queue_disposition(self, category: &'static str) -> Disposition {
        match self {
            Self::Completed | Self::Rejected => Disposition::Complete,
            Self::Unprocessable => Disposition::Park(error_class(category)),
            Self::Retained | Self::Contended => Disposition::Retry(error_class(category)),
        }
    }

    const fn metric(self) -> IngestionMetricOutcome {
        match self {
            Self::Completed => IngestionMetricOutcome::Completed,
            Self::Rejected => IngestionMetricOutcome::Rejected,
            Self::Unprocessable => IngestionMetricOutcome::Unprocessable,
            Self::Retained | Self::Contended => IngestionMetricOutcome::Failed,
        }
    }
}

/// `failure` carries the safe cause facts of a retained failure; `category` is then its
/// stage-level code.
fn log_attempt(
    disposition: AttemptDisposition,
    category: &'static str,
    failure: Option<&FailureDiagnostic>,
    command: Option<&CommandCorrelation>,
    job: &Job,
    duration_ms: u128,
) {
    let dependency = failure.and_then(FailureDiagnostic::dependency);
    let error_category = failure.map(FailureDiagnostic::category);
    let sqlstate = failure.and_then(FailureDiagnostic::sqlstate);
    let job_id = tracing::field::display(job.id());
    let attempt = job.attempt();
    // tracing needs a literal level and message per call site; this writes the shared
    // correlation fields once for every disposition.
    macro_rules! log_with_command {
        ($level:ident, $category:ident, $message:literal) => {
            match command {
                Some(command) => $level!(
                    $category = category,
                    dependency,
                    error_category,
                    sqlstate,
                    job_id,
                    attempt,
                    duration_ms,
                    command_id = %command.command_id,
                    submission_id = %command.submission_id,
                    listing_source_id = %command.listing_source_id,
                    request_id = command.request_id.as_deref().unwrap_or(""),
                    correlation_id = command.correlation_id.as_deref().unwrap_or(""),
                    operation = command.operation,
                    item_index = command.index,
                    $message
                ),
                None => $level!(
                    $category = category,
                    dependency,
                    error_category,
                    sqlstate,
                    job_id,
                    attempt,
                    duration_ms,
                    $message
                ),
            }
        };
    }
    disposition.metric().emit();
    match disposition {
        AttemptDisposition::Completed => {
            log_with_command!(info, outcome, "Ingestion command completed")
        }
        AttemptDisposition::Rejected => {
            log_with_command!(warn, error_code, "Ingestion command rejected without retry")
        }
        AttemptDisposition::Unprocessable => {
            log_with_command!(
                warn,
                error_code,
                "Ingestion command cannot apply as delivered"
            )
        }
        AttemptDisposition::Retained => {
            log_with_command!(warn, error_code, "Ingestion command retained for retry")
        }
        AttemptDisposition::Contended => {
            log_with_command!(
                info,
                error_code,
                "Ingestion command retained after a concurrent write"
            )
        }
    }
}

// Never log arbitrary trace text (including raw keys or line breaks). Suppress values that
// cannot be represented safely rather than truncating them into misleading correlations.
fn safe_trace_id(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(value)
}

struct CommandCorrelation {
    command_id: String,
    submission_id: String,
    listing_source_id: String,
    request_id: Option<String>,
    correlation_id: Option<String>,
    operation: &'static str,
    index: usize,
}

#[derive(Clone, Copy)]
enum IngestionMetricOutcome {
    Completed,
    Rejected,
    Unprocessable,
    Failed,
}

impl IngestionMetricOutcome {
    // The shared tracing formatter nests event fields under `fields`, while the host monitoring's
    // metric filter matches a top-level `$.ingestion_outcome`, bypassing the log level. Keep this
    // one JSON line static and payload-free.
    const fn json(self) -> &'static str {
        match self {
            Self::Completed => r#"{"ingestion_outcome":"completed"}"#,
            Self::Rejected => r#"{"ingestion_outcome":"rejected"}"#,
            Self::Unprocessable => r#"{"ingestion_outcome":"unprocessable"}"#,
            Self::Failed => r#"{"ingestion_outcome":"failed"}"#,
        }
    }

    fn emit(self) {
        println!("{}", self.json());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobDisposition {
    Complete(&'static str),
    Rejected(&'static str),
    Unprocessable(&'static str),
    /// A retryable failure; its stage is the error's stage-level diagnostic code.
    Failed(FailureDiagnostic),
    /// A concurrent writer won a race; retried without a failure diagnostic.
    Contended(&'static str),
}

/// Only a verified decoded command supplies correlation fields.
async fn process_ingestion_job(
    body: &str,
    use_case: &(dyn ProcessProductListingIngestionUseCase + Send + Sync),
) -> (Option<CommandCorrelation>, JobDisposition) {
    let envelope =
        match codec::decode(body).and_then(codec::IngestionEnvelopeV2::into_service_envelope) {
            Ok(envelope) => envelope,
            Err(_) => return (None, JobDisposition::Unprocessable("invalid_wire_command")),
        };
    let metadata = &envelope.message.metadata;
    let correlation = CommandCorrelation {
        command_id: metadata.command_id.as_str().to_owned(),
        submission_id: metadata.submission_id.as_str().to_owned(),
        listing_source_id: metadata.listing_source_id.to_string(),
        request_id: safe_trace_id(metadata.request_id.as_str()).map(str::to_owned),
        correlation_id: safe_trace_id(metadata.correlation_id.as_str()).map(str::to_owned),
        operation: metadata.operation.as_str(),
        index: metadata.index,
    };
    let disposition = match use_case.execute(envelope).await {
        Ok(ProductListingIngestionCompletion::Applied(_)) => JobDisposition::Complete("applied"),
        Ok(ProductListingIngestionCompletion::AlreadyCompleted) => {
            JobDisposition::Complete("already_completed")
        }
        Err(error) => match error.failure_class() {
            ProductListingIngestionFailureClass::BusinessRejection => {
                JobDisposition::Rejected(error.diagnostic_code())
            }
            ProductListingIngestionFailureClass::Unprocessable => {
                JobDisposition::Unprocessable(error.diagnostic_code())
            }
            // Every retryable ingestion step reads or writes PostgreSQL.
            ProductListingIngestionFailureClass::Retryable => {
                JobDisposition::Failed(FailureDiagnostic::postgres(error.diagnostic_code(), &error))
            }
            ProductListingIngestionFailureClass::Contended => {
                JobDisposition::Contended(error.diagnostic_code())
            }
        },
    };
    (Some(correlation), disposition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::fifo_job;
    use product_listing_service::{
        ports::{ProductListingCommandReceiptError, ProductListingRawStreamId},
        use_cases::{
            CaptureProductListingRawObservationResult, ProductListingIngestionEffect,
            ProductListingIngestionEnvelope, ProductListingIngestionError,
            UpdateProductListingError,
        },
    };
    use serde_json::{Map, Value};
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const GOLDEN: &str =
        include_str!("../../../product-listing-ingestion-queue/tests/fixtures/ingestion_v2.json");

    #[derive(Clone, Copy)]
    enum Mode {
        Applied,
        Complete,
        Failure,
        CommitUnknown,
        CommitConnectionLost,
        VersionConflict,
        MissingTarget,
        Forbidden,
        ReceiptFailure,
        Conflict,
    }

    struct FakeUseCase {
        modes: Mutex<VecDeque<Mode>>,
        calls: AtomicUsize,
        command_ids: Mutex<Vec<String>>,
    }

    impl FakeUseCase {
        fn new(modes: impl IntoIterator<Item = Mode>) -> Self {
            Self {
                modes: Mutex::new(modes.into_iter().collect()),
                calls: AtomicUsize::new(0),
                command_ids: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ProcessProductListingIngestionUseCase for FakeUseCase {
        async fn execute(
            &self,
            envelope: ProductListingIngestionEnvelope,
        ) -> Result<ProductListingIngestionCompletion, ProductListingIngestionError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.command_ids
                .lock()
                .unwrap()
                .push(envelope.message.metadata.command_id.as_str().to_owned());
            let mode = self
                .modes
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected use case invocation");
            match mode {
                Mode::Applied => Ok(ProductListingIngestionCompletion::Applied(
                    ProductListingIngestionEffect::RawCaptured(
                        CaptureProductListingRawObservationResult::Duplicate {
                            product_listing_raw_stream_id: ProductListingRawStreamId::new(),
                            latest_revision: 1,
                        },
                    ),
                )),
                Mode::Complete => Ok(ProductListingIngestionCompletion::AlreadyCompleted),
                Mode::Failure => Err(ProductListingIngestionError::BeginTransactionFailed(
                    application::error::static_error("test transaction failure"),
                )),
                Mode::CommitUnknown => Err(ProductListingIngestionError::CommitTransactionFailed(
                    application::error::static_error("test transaction failure"),
                )),
                Mode::CommitConnectionLost => Err(
                    ProductListingIngestionError::CommitTransactionFailed(Box::new(
                        sqlx::Error::Io(std::io::Error::other("SENTINEL_ERROR_TEXT")),
                    )),
                ),
                Mode::VersionConflict => Err(ProductListingIngestionError::Update(
                    UpdateProductListingError::ConcurrencyConflict,
                )),
                Mode::MissingTarget => Err(ProductListingIngestionError::Update(
                    UpdateProductListingError::NotFound,
                )),
                Mode::Forbidden => Err(ProductListingIngestionError::Update(
                    UpdateProductListingError::Forbidden,
                )),
                Mode::ReceiptFailure => Err(ProductListingIngestionError::Receipt(
                    ProductListingCommandReceiptError::OperationFailed {
                        source: Box::new(std::io::Error::other("SENTINEL_ERROR_TEXT")),
                    },
                )),
                Mode::Conflict => Err(ProductListingIngestionError::FingerprintConflict),
            }
        }
    }

    fn golden_body(index: usize) -> String {
        let values: Vec<serde_json::Value> = serde_json::from_str(GOLDEN).unwrap();
        values[index].to_string()
    }

    fn valid_body() -> String {
        golden_body(0)
    }

    fn job(body: &str) -> Job {
        fifo_job(body, "listing", 1)
    }

    fn handler(use_case: FakeUseCase) -> (Arc<FakeUseCase>, ProductListingIngestionJobs) {
        let use_case = Arc::new(use_case);
        let jobs = ProductListingIngestionJobs::new(use_case.clone());
        (use_case, jobs)
    }

    /// The single attempt event an ingestion job logged.
    fn attempt_event(logs: &CapturedLogs, message: &str) -> Map<String, Value> {
        let mut events = logs.events_where("message", message);
        assert_eq!(1, events.len(), "{message}");
        events.remove(0)
    }

    fn assert_duration(fields: &Map<String, Value>) {
        let duration = &fields["duration_ms"];
        assert!(
            duration.as_u64().is_some()
                || duration.as_str().is_some_and(|v| v.parse::<u64>().is_ok()),
            "{duration:?}"
        );
    }

    #[test]
    fn metric_lines_have_only_the_root_level_cdk_outcome_field() {
        for (outcome, value) in [
            (IngestionMetricOutcome::Completed, "completed"),
            (IngestionMetricOutcome::Rejected, "rejected"),
            (IngestionMetricOutcome::Unprocessable, "unprocessable"),
            (IngestionMetricOutcome::Failed, "failed"),
        ] {
            let object: serde_json::Value = serde_json::from_str(outcome.json()).unwrap();
            assert_eq!(object.as_object().unwrap().len(), 1);
            assert_eq!(object["ingestion_outcome"].as_str(), Some(value));
        }
    }

    #[test]
    fn only_bounded_trace_identifiers_can_be_logged() {
        for value in ["fixture-request", "correlation_123"] {
            assert_eq!(safe_trace_id(value), Some(value));
        }
        for value in [
            "",
            "SENTINEL_PAYLOAD\n",
            "raw/key",
            "token:secret",
            "x y",
            "ü",
        ] {
            assert_eq!(safe_trace_id(value), None);
        }
        assert_eq!(safe_trace_id(&"x".repeat(129)), None);
    }

    #[tokio::test]
    async fn should_map_every_ingestion_outcome_to_its_queue_disposition() {
        let create = golden_body(0);
        let update = golden_body(1);
        for (mode, body, expected) in [
            (Mode::Applied, &create, Disposition::Complete),
            (Mode::Complete, &create, Disposition::Complete),
            (Mode::MissingTarget, &update, Disposition::Complete),
            (Mode::Forbidden, &update, Disposition::Complete),
            (
                Mode::Conflict,
                &create,
                Disposition::Park(error_class("FINGERPRINT_CONFLICT")),
            ),
            (
                Mode::Failure,
                &create,
                Disposition::Retry(error_class("BEGIN_TRANSACTION_FAILED")),
            ),
            (
                Mode::CommitUnknown,
                &create,
                Disposition::Retry(error_class("COMMIT_UNCONFIRMED")),
            ),
            (
                Mode::ReceiptFailure,
                &update,
                Disposition::Retry(error_class("RECEIPT_OPERATION_FAILED")),
            ),
            (
                Mode::VersionConflict,
                &update,
                Disposition::Retry(error_class("UPDATE_CONCURRENCY_CONFLICT")),
            ),
        ] {
            let (use_case, jobs) = handler(FakeUseCase::new([mode]));
            assert_eq!(expected, jobs.handle(&job(body)).await, "{expected:?}");
            assert_eq!(1, use_case.calls());
        }
    }

    #[tokio::test]
    async fn should_park_unprocessable_commands_on_every_attempt_when_they_cannot_apply() {
        let body = valid_body();
        // The queue releases a parked first delivery and moves a redelivery to custody; the
        // handler reports the same disposition either way.
        for attempt in [1, 2] {
            let (use_case, jobs) = handler(FakeUseCase::new([Mode::Conflict]));
            assert_eq!(
                Disposition::Park(error_class("FINGERPRINT_CONFLICT")),
                jobs.handle(&fifo_job(&body, "listing", attempt)).await
            );
            assert_eq!(1, use_case.calls());

            let (logs, guard) = CapturedLogs::start();
            let (no_calls, jobs) = handler(FakeUseCase::new([]));
            assert_eq!(
                Disposition::Park(error_class("invalid_wire_command")),
                jobs.handle(&fifo_job("not-json", "other", attempt)).await
            );
            drop(guard);
            assert_eq!(0, no_calls.calls());
            assert!(!logs.text().contains("not-json"));
        }

        let (logs, guard) = CapturedLogs::start();
        let (_, jobs) = handler(FakeUseCase::new([Mode::Conflict]));
        jobs.handle(&job(&body)).await;
        drop(guard);
        let conflict = attempt_event(&logs, "Ingestion command cannot apply as delivered");
        assert_eq!(
            Some("FINGERPRINT_CONFLICT"),
            conflict["error_code"].as_str()
        );
        assert_eq!(
            Some(codec::decode(&body).unwrap().command_id()),
            conflict["command_id"].as_str()
        );
    }

    #[tokio::test]
    async fn captured_logs_correlate_only_confirmed_completions_and_attempted_failures() {
        let create = golden_body(0);
        let update = golden_body(1);
        let (use_case, jobs) = handler(FakeUseCase::new([
            Mode::Applied,
            Mode::Complete,
            Mode::ReceiptFailure,
        ]));
        for (body, message, outcome, error_code, item_index) in [
            (
                &create,
                "Ingestion command completed",
                Some("applied"),
                None,
                0,
            ),
            (
                &create,
                "Ingestion command completed",
                Some("already_completed"),
                None,
                0,
            ),
            (
                &update,
                "Ingestion command retained for retry",
                None,
                Some("RECEIPT_OPERATION_FAILED"),
                1,
            ),
        ] {
            let (logs, guard) = CapturedLogs::start();
            let job = job(body);
            jobs.handle(&job).await;
            drop(guard);

            let fields = attempt_event(&logs, message);
            assert_eq!(
                Some(job.id().to_string().as_str()),
                fields["job_id"].as_str()
            );
            assert_eq!(Some(1), fields["attempt"].as_u64());
            assert_eq!(outcome, fields.get("outcome").and_then(Value::as_str));
            assert_eq!(error_code, fields.get("error_code").and_then(Value::as_str));
            assert_duration(&fields);
            let envelope = codec::decode(body).unwrap();
            assert_eq!(Some(envelope.command_id()), fields["command_id"].as_str());
            assert_eq!(
                Some(envelope.submission_id()),
                fields["submission_id"].as_str()
            );
            assert_eq!(
                Some("ls_01h455vb4pex5vy7enb1p677vn"),
                fields["listing_source_id"].as_str()
            );
            assert_eq!(Some("fixture-request"), fields["request_id"].as_str());
            assert_eq!(
                Some("fixture-correlation"),
                fields["correlation_id"].as_str()
            );
            assert_eq!(
                Some(envelope.operation().as_str()),
                fields["operation"].as_str()
            );
            assert_eq!(Some(item_index), fields["item_index"].as_u64());
            assert!(!logs.text().contains("SENTINEL_ERROR_TEXT"));
        }
        assert_eq!(3, use_case.calls());
    }

    #[tokio::test]
    async fn should_acknowledge_business_rejections_without_blocking_successors() {
        let update = golden_body(1);
        for (mode, expected) in [
            (Mode::MissingTarget, "UPDATE_NOT_FOUND"),
            (Mode::Forbidden, "UPDATE_FORBIDDEN"),
        ] {
            let (logs, guard) = CapturedLogs::start();
            let (_, jobs) = handler(FakeUseCase::new([mode]));
            let disposition = jobs.handle(&job(&update)).await;
            drop(guard);

            assert_eq!(Disposition::Complete, disposition, "{expected}");
            let rejection = attempt_event(&logs, "Ingestion command rejected without retry");
            assert_eq!(Some(expected), rejection["error_code"].as_str());
            assert_eq!(
                Some(codec::decode(&update).unwrap().command_id()),
                rejection["command_id"].as_str()
            );
            assert!(!logs.text().contains("SENTINEL_ERROR_TEXT"));
        }
    }

    #[tokio::test]
    async fn retains_a_contended_command_without_a_dependency_failure() {
        let (logs, guard) = CapturedLogs::start();
        let (_, jobs) = handler(FakeUseCase::new([Mode::VersionConflict]));

        let disposition = jobs.handle(&job(&golden_body(1))).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("UPDATE_CONCURRENCY_CONFLICT")),
            disposition
        );
        let contended = attempt_event(&logs, "Ingestion command retained after a concurrent write");
        assert_eq!(
            Some("UPDATE_CONCURRENCY_CONFLICT"),
            contended["error_code"].as_str()
        );
        assert!(!contended.contains_key("dependency"));
        assert!(!contended.contains_key("error_category"));
        assert!(
            logs.events_where("message", "Ingestion command retained for retry")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn captured_logs_distinguish_receipt_and_unconfirmed_commit_without_causes() {
        let body = golden_body(1);
        for (mode, expected, error_category) in [
            (
                Mode::ReceiptFailure,
                "RECEIPT_OPERATION_FAILED",
                "unclassified",
            ),
            (Mode::CommitUnknown, "COMMIT_UNCONFIRMED", "unclassified"),
            (Mode::CommitConnectionLost, "COMMIT_UNCONFIRMED", "io"),
        ] {
            let (logs, guard) = CapturedLogs::start();
            let (_, jobs) = handler(FakeUseCase::new([mode]));
            let job = job(&body);
            let disposition = jobs.handle(&job).await;
            drop(guard);

            assert_eq!(Disposition::Retry(error_class(expected)), disposition);
            let failure = attempt_event(&logs, "Ingestion command retained for retry");
            assert_eq!(Some(expected), failure["error_code"].as_str());
            assert_eq!(
                Some(job.id().to_string().as_str()),
                failure["job_id"].as_str()
            );
            assert!(!failure.contains_key("outcome"));
            assert_eq!(Some("postgres"), failure["dependency"].as_str());
            assert_eq!(Some(error_category), failure["error_category"].as_str());
            assert_duration(&failure);
            assert!(
                logs.events_where("outcome", "applied").is_empty(),
                "{expected}"
            );
            assert!(!logs.text().contains("SENTINEL_ERROR_TEXT"));
        }
    }

    #[tokio::test]
    async fn captured_logs_drop_forged_and_unsafe_trace_values_and_never_log_payloads() {
        let mut body: serde_json::Value = serde_json::from_str(&valid_body()).unwrap();
        body["requestId"] = serde_json::json!("SENTINEL_PAYLOAD\n");
        body["correlationId"] = serde_json::json!("x".repeat(129));
        let unsafe_trace = body.to_string();
        let (logs, guard) = CapturedLogs::start();
        let (_, jobs) = handler(FakeUseCase::new([Mode::Complete]));
        let disposition = jobs.handle(&job(&unsafe_trace)).await;
        drop(guard);
        assert_eq!(Disposition::Complete, disposition);
        let completed = attempt_event(&logs, "Ingestion command completed");
        assert_eq!(Some("already_completed"), completed["outcome"].as_str());
        assert_eq!(Some(""), completed["request_id"].as_str());
        assert_eq!(Some(""), completed["correlation_id"].as_str());
        assert!(completed.contains_key("command_id"));
        assert!(!logs.text().contains("SENTINEL_PAYLOAD"));
        assert!(!logs.text().contains("Blue vase"));

        body["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        let forged = body.to_string();
        let (logs, guard) = CapturedLogs::start();
        let (no_calls, jobs) = handler(FakeUseCase::new([]));
        let disposition = jobs.handle(&job(&forged)).await;
        drop(guard);
        assert_eq!(
            Disposition::Park(error_class("invalid_wire_command")),
            disposition
        );
        assert_eq!(0, no_calls.calls());
        let failure = attempt_event(&logs, "Ingestion command cannot apply as delivered");
        assert_eq!(Some("invalid_wire_command"), failure["error_code"].as_str());
        assert!(!failure.contains_key("command_id"));
        assert!(!failure.contains_key("listing_source_id"));
        assert_duration(&failure);
        for secret in ["SENTINEL_PAYLOAD", "Blue vase", "SENTINEL_ERROR_TEXT"] {
            assert!(!logs.text().contains(secret));
        }
    }

    #[tokio::test]
    async fn captured_malformed_command_has_no_command_fields() {
        let (logs, guard) = CapturedLogs::start();
        let (no_calls, jobs) = handler(FakeUseCase::new([]));
        let job = job("SENTINEL_MALFORMED_PAYLOAD");
        let disposition = jobs.handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Park(error_class("invalid_wire_command")),
            disposition
        );
        assert_eq!(0, no_calls.calls());
        let failure = attempt_event(&logs, "Ingestion command cannot apply as delivered");
        assert_eq!(Some("invalid_wire_command"), failure["error_code"].as_str());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            failure["job_id"].as_str()
        );
        assert_duration(&failure);
        for key in [
            "command_id",
            "submission_id",
            "listing_source_id",
            "request_id",
            "correlation_id",
            "operation",
            "item_index",
        ] {
            assert!(!failure.contains_key(key), "{key}");
        }
        assert!(!logs.text().contains("SENTINEL_MALFORMED_PAYLOAD"));
    }

    #[tokio::test]
    async fn only_verified_decoded_commands_supply_correlation_and_specific_codes() {
        let invalid = FakeUseCase::new([]);
        let (correlation, disposition) = process_ingestion_job("not-json", &invalid).await;
        assert_eq!(
            disposition,
            JobDisposition::Unprocessable("invalid_wire_command")
        );
        assert!(correlation.is_none());
        let mut tampered: serde_json::Value = serde_json::from_str(&valid_body()).unwrap();
        tampered["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        let (correlation, disposition) =
            process_ingestion_job(&tampered.to_string(), &invalid).await;
        assert_eq!(
            disposition,
            JobDisposition::Unprocessable("invalid_wire_command")
        );
        assert!(correlation.is_none());
        assert_eq!(0, invalid.calls());

        let body = golden_body(1);
        let decoded = codec::decode(&body).unwrap();
        let use_case = FakeUseCase::new([Mode::MissingTarget]);
        let (correlation, disposition) = process_ingestion_job(&body, &use_case).await;
        assert_eq!(disposition, JobDisposition::Rejected("UPDATE_NOT_FOUND"));
        let command = correlation.unwrap();
        assert_eq!(command.command_id, decoded.command_id());
        assert_eq!(command.submission_id, decoded.submission_id());
        assert_eq!(command.operation, decoded.operation().as_str());
        assert_eq!(command.index, decoded.index());

        let use_case = FakeUseCase::new([Mode::Conflict]);
        let (correlation, disposition) = process_ingestion_job(&body, &use_case).await;
        assert_eq!(
            disposition,
            JobDisposition::Unprocessable("FINGERPRINT_CONFLICT")
        );
        assert!(correlation.is_some());
    }

    #[tokio::test]
    async fn valid_envelope_and_duplicate_completion_are_the_only_acknowledgments() {
        let body = valid_body();
        let (use_case, jobs) = handler(FakeUseCase::new([Mode::Applied, Mode::Complete]));
        assert_eq!(Disposition::Complete, jobs.handle(&job(&body)).await);
        assert_eq!(Disposition::Complete, jobs.handle(&job(&body)).await);
        assert_eq!(use_case.calls(), 2);
        let expected = codec::decode(&body).unwrap().command_id().to_owned();
        assert_eq!(
            *use_case.command_ids.lock().unwrap(),
            [expected.clone(), expected]
        );
    }

    #[tokio::test]
    async fn replayed_receipt_then_distinct_command_can_complete_after_lost_response() {
        let old = golden_body(0);
        let new = golden_body(1);
        assert_ne!(
            codec::decode(&old).unwrap().command_id(),
            codec::decode(&new).unwrap().command_id()
        );
        // The previous attempt committed `old` but lost its settlement. The service recognizes
        // its receipt on redelivery before a new command is attempted.
        let (use_case, jobs) = handler(FakeUseCase::new([Mode::Complete, Mode::Applied]));
        assert_eq!(
            Disposition::Complete,
            jobs.handle(&fifo_job(&old, "listing", 2)).await
        );
        assert_eq!(Disposition::Complete, jobs.handle(&job(&new)).await);
        assert_eq!(
            *use_case.command_ids.lock().unwrap(),
            [
                codec::decode(&old).unwrap().command_id().to_owned(),
                codec::decode(&new).unwrap().command_id().to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn malformed_wire_stops_before_any_service_call() {
        let mut tampered: serde_json::Value = serde_json::from_str(&valid_body()).unwrap();
        tampered["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        for invalid in ["not-json".to_owned(), "{}".to_owned(), tampered.to_string()] {
            let (use_case, jobs) = handler(FakeUseCase::new([Mode::Complete]));
            assert_eq!(
                Disposition::Park(error_class("invalid_wire_command")),
                jobs.handle(&job(&invalid)).await
            );
            assert_eq!(use_case.calls(), 0);
        }
    }
}
