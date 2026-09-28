use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, handle_fifo_sqs_invocation, process_fifo_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_ingestion_sqs::codec;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingCommandReceiptStoreFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    ProcessProductListingIngestionHandler, ProcessProductListingIngestionUseCase,
    ProductListingIngestionCompletion,
};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    sync::Arc,
    time::Duration,
};
use tracing::{info, warn};

const COMPONENT: &str = "product-listing-ingestion-lambda";
const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(35);

/// Compose adapters only; the service use case owns the transaction and command receipt.
pub fn compose_ingestion_use_case(
    pool: sqlx::PgPool,
) -> Arc<dyn ProcessProductListingIngestionUseCase> {
    Arc::new(ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
        SqlxAuctionReferenceValidatorFactory::new(),
        SqlxProductListingRawCaptureWriterFactory::new(),
        SqlxProductListingCommandReceiptStoreFactory::new(),
    ))
}

pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

/// Validate all SQS IDs before bounded credential refresh and composition. An incomplete setup
/// leaves every record for native retry; no transport receipt is used as command identity.
pub async fn handle_invocation<F, Fut>(
    event: LambdaEvent<SqsEvent>,
    setup: F,
) -> Result<SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Arc<dyn ProcessProductListingIngestionUseCase>, Error>>,
{
    let record_count = event.payload.records.len();
    if record_count == 0 {
        return Ok(SqsBatchResponse::default());
    }
    let setup_ready = Cell::new(false);
    let response = handle_fifo_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| {
            setup_ready.set(true);
            async move { handler_with_invocation_budget(event, use_case.as_ref(), &budget).await }
        },
    )
    .await;
    if !setup_ready.get()
        && let Ok(response) = &response
    {
        for failure in &response.batch_item_failures {
            log_unprocessed(&failure.item_identifier);
        }
    }
    response
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProcessProductListingIngestionUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProcessProductListingIngestionUseCase + Send + Sync),
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

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProcessProductListingIngestionUseCase + Send + Sync),
    remaining: Duration,
    per_record_cap: Duration,
) -> Result<SqsBatchResponse, Error> {
    let record_count = event.payload.records.len();
    let mut attempted = 0;
    let correlation = RefCell::new(None);
    let response = process_fifo_batch(
        event,
        remaining,
        per_record_cap,
        |body| {
            let correlation = &correlation;
            async move { process_ingestion_job(&body, use_case, correlation).await }
        },
        |attempt| {
            attempted += 1;
            let (complete, category) = match &attempt.outcome {
                RecordOutcome::Completed(JobDisposition::Complete(category)) => (true, *category),
                RecordOutcome::Completed(JobDisposition::Failed(category)) => (false, *category),
                RecordOutcome::MissingBody => (false, "missing_message_body"),
                RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
                RecordOutcome::TimedOut => (false, "execution_timeout"),
                RecordOutcome::Panicked => (false, "handler_panicked"),
            };
            let command = correlation.borrow_mut().take();
            let sqs_message_id = safe_sqs_message_id(&attempt.message_id).unwrap_or("invalid");
            let duration_ms = attempt.duration.as_millis();
            match (complete, command.as_ref()) {
                (true, Some(command)) => {
                    IngestionMetricOutcome::Completed.emit();
                    info!(
                        outcome = category,
                        sqs_message_id,
                        duration_ms,
                        command_id = %command.command_id,
                        submission_id = %command.submission_id,
                        listing_source_id = %command.listing_source_id,
                        request_id = command.request_id.as_deref().unwrap_or(""),
                        correlation_id = command.correlation_id.as_deref().unwrap_or(""),
                        operation = command.operation,
                        item_index = command.index,
                        "Ingestion command completed"
                    );
                }
                (true, None) => {
                    IngestionMetricOutcome::Completed.emit();
                    info!(
                        outcome = category,
                        sqs_message_id, duration_ms, "Ingestion command completed"
                    );
                }
                (false, Some(command)) => {
                    IngestionMetricOutcome::Failed.emit();
                    warn!(
                        error_code = category,
                        sqs_message_id,
                        duration_ms,
                        command_id = %command.command_id,
                        submission_id = %command.submission_id,
                        listing_source_id = %command.listing_source_id,
                        request_id = command.request_id.as_deref().unwrap_or(""),
                        correlation_id = command.correlation_id.as_deref().unwrap_or(""),
                        operation = command.operation,
                        item_index = command.index,
                        "Ingestion command retained for retry"
                    );
                }
                (false, None) => {
                    IngestionMetricOutcome::Failed.emit();
                    warn!(
                        error_code = category,
                        sqs_message_id, duration_ms, "Ingestion command retained for retry"
                    );
                }
            }
            complete
        },
    )
    .await?;
    let unprocessed_count = record_count - attempted;
    let suffix_start = response.batch_item_failures.len() - unprocessed_count;
    for failure in response.batch_item_failures.iter().skip(suffix_start) {
        log_unprocessed(&failure.item_identifier);
    }
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished ProductListing ingestion batch"
    );
    Ok(response)
}

// SQS supplies UUID message IDs; don't interpolate arbitrary event strings into diagnostics.
// The original ID is still returned unchanged in the partial-batch response.
fn safe_sqs_message_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(id)
}

fn log_unprocessed(message_id: &str) {
    IngestionMetricOutcome::Unprocessed.emit();
    let sqs_message_id = safe_sqs_message_id(message_id).unwrap_or("invalid");
    info!(
        outcome = "unprocessed",
        sqs_message_id,
        duration_ms = 0_u64,
        "Ingestion record retained without execution"
    );
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
    Failed,
    Unprocessed,
}

impl IngestionMetricOutcome {
    // The shared tracing formatter nests event fields under `fields`, while the CDK metric
    // filters match `$.ingestion_outcome`. Keep this one top-level JSON line static and payload-free.
    const fn json(self) -> &'static str {
        match self {
            Self::Completed => r#"{"ingestion_outcome":"completed"}"#,
            Self::Failed => r#"{"ingestion_outcome":"failed"}"#,
            Self::Unprocessed => r#"{"ingestion_outcome":"unprocessed"}"#,
        }
    }

    fn emit(self) {
        println!("{}", self.json());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobDisposition {
    Complete(&'static str),
    Failed(&'static str),
}

async fn process_ingestion_job(
    body: &str,
    use_case: &(dyn ProcessProductListingIngestionUseCase + Send + Sync),
    correlation: &RefCell<Option<CommandCorrelation>>,
) -> JobDisposition {
    let envelope =
        match codec::decode(body).and_then(codec::IngestionEnvelopeV1::into_service_envelope) {
            Ok(envelope) => envelope,
            Err(_) => return JobDisposition::Failed("invalid_wire_command"),
        };
    let metadata = &envelope.message.metadata;
    correlation.replace(Some(CommandCorrelation {
        command_id: metadata.command_id.as_str().to_owned(),
        submission_id: metadata.submission_id.as_str().to_owned(),
        listing_source_id: metadata.listing_source_id.to_string(),
        request_id: safe_trace_id(metadata.request_id.as_str()).map(str::to_owned),
        correlation_id: safe_trace_id(metadata.correlation_id.as_str()).map(str::to_owned),
        operation: metadata.operation.as_str(),
        index: metadata.index,
    }));
    match use_case.execute(envelope).await {
        Ok(ProductListingIngestionCompletion::Applied(_)) => JobDisposition::Complete("applied"),
        Ok(ProductListingIngestionCompletion::AlreadyCompleted) => {
            JobDisposition::Complete("already_completed")
        }
        Err(error) => JobDisposition::Failed(error.diagnostic_code()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::{
        ports::{ProductListingCommandReceiptError, ProductListingRawStreamId},
        use_cases::{
            CaptureProductListingRawObservationResult, ProductListingIngestionEffect,
            ProductListingIngestionEnvelope, ProductListingIngestionError,
            UpdateProductListingError,
        },
    };
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span::{Attributes, Id, Record},
        subscriber::Interest,
    };

    const GOLDEN: &str =
        include_str!("../../product-listing-ingestion-sqs/tests/fixtures/ingestion_v1.json");

    #[derive(Clone, Copy)]
    enum Mode {
        Applied,
        Complete,
        Failure,
        CommitUnknown,
        MissingTarget,
        Forbidden,
        ReceiptFailure,
        Conflict,
        Panic,
        Wait,
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
                Mode::Failure => Err(ProductListingIngestionError::BeginTransactionFailed),
                Mode::CommitUnknown => Err(ProductListingIngestionError::CommitTransactionFailed),
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
                Mode::Panic => panic!("private use case detail"),
                Mode::Wait => std::future::pending().await,
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

    fn event(records: &[(Option<&str>, Option<&str>)]) -> LambdaEvent<SqsEvent> {
        let mut payload = SqsEvent::default();
        payload.records = records
            .iter()
            .map(|(id, body)| {
                let mut record = SqsMessage::default();
                record.message_id = id.map(str::to_owned);
                record.body = body.map(str::to_owned);
                record
            })
            .collect();
        let mut context = Context::default();
        context.deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 45_000;
        LambdaEvent::new(payload, context)
    }

    fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|f| f.item_identifier)
            .collect()
    }

    // Capture real tracing events without changing production dependencies or the JSON metric line.
    struct TraceFields(BTreeMap<String, String>);

    impl Visit for TraceFields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }
    }

    struct TraceSink(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

    impl Subscriber for TraceSink {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            if event.metadata().target() == "product_listing_ingestion_lambda" {
                let mut fields = TraceFields(BTreeMap::new());
                event.record(&mut fields);
                self.0.lock().unwrap().push(fields.0);
            }
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::always()
        }
    }

    #[test]
    fn metric_lines_have_only_the_root_level_cdk_outcome_field() {
        for (outcome, value) in [
            (IngestionMetricOutcome::Completed, "completed"),
            (IngestionMetricOutcome::Failed, "failed"),
            (IngestionMetricOutcome::Unprocessed, "unprocessed"),
        ] {
            let object: serde_json::Value = serde_json::from_str(outcome.json()).unwrap();
            assert_eq!(object.as_object().unwrap().len(), 1);
            assert_eq!(object["ingestion_outcome"].as_str(), Some(value));
        }
    }

    #[test]
    fn only_bounded_sqs_identifiers_are_written_to_diagnostics() {
        for id in ["sqs-1", "123e4567-e89b-12d3-a456-426614174000"] {
            assert_eq!(safe_sqs_message_id(id), Some(id));
        }
        for id in ["", "a\nsecret", "arn:secret", "with space"] {
            assert_eq!(safe_sqs_message_id(id), None);
        }
        assert_eq!(safe_sqs_message_id(&"x".repeat(129)), None);
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
    async fn captured_logs_correlate_only_confirmed_completions_and_attempted_failures() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
        let create = golden_body(0);
        let update = golden_body(1);
        let use_case = FakeUseCase::new([Mode::Applied, Mode::Complete, Mode::MissingTarget]);
        let response = handler(
            event(&[
                (Some("sqs-1"), Some(&create)),
                (Some("sqs-2"), Some(&create)),
                (Some("sqs-3"), Some(&update)),
                (Some("sqs-4"), Some(&create)),
            ]),
            &use_case,
        )
        .await
        .unwrap();
        drop(guard);
        assert_eq!(failure_ids(response), ["sqs-3", "sqs-4"]);
        assert_eq!(use_case.calls(), 3);
        let records = events.lock().unwrap();
        let attempted: Vec<_> = records
            .iter()
            .filter(|fields| {
                fields
                    .get("message")
                    .is_some_and(|m| m.contains("Ingestion command"))
            })
            .collect();
        assert_eq!(attempted.len(), 3);
        let expected = [
            ("sqs-1", "applied", None),
            ("sqs-2", "already_completed", None),
            ("sqs-3", "", Some("UPDATE_NOT_FOUND")),
        ];
        for (fields, (sqs_id, outcome, error_code)) in attempted.iter().zip(expected) {
            assert_eq!(
                fields.get("sqs_message_id").map(String::as_str),
                Some(sqs_id)
            );
            assert_eq!(
                fields.get("outcome").map(String::as_str).unwrap_or(""),
                outcome
            );
            assert_eq!(fields.get("error_code").map(String::as_str), error_code);
            assert!(fields.get("duration_ms").unwrap().parse::<u64>().is_ok());
            let envelope =
                codec::decode(if sqs_id == "sqs-3" { &update } else { &create }).unwrap();
            assert_eq!(
                fields.get("command_id").map(String::as_str),
                Some(envelope.command_id())
            );
            assert_eq!(
                fields.get("submission_id").map(String::as_str),
                Some(envelope.submission_id())
            );
            assert_eq!(
                fields.get("listing_source_id").map(String::as_str),
                Some("ls_01h455vb4pex5vy7enb1p677vn")
            );
            assert_eq!(
                fields.get("request_id").map(String::as_str),
                Some("fixture-request")
            );
            assert_eq!(
                fields.get("correlation_id").map(String::as_str),
                Some("fixture-correlation")
            );
            assert_eq!(
                fields.get("operation").map(String::as_str),
                Some(envelope.operation().as_str())
            );
            assert_eq!(
                fields.get("item_index").map(String::as_str),
                Some(if sqs_id == "sqs-3" { "1" } else { "0" })
            );
        }
        let unprocessed = records
            .iter()
            .find(|fields| fields.get("outcome").is_some_and(|v| v == "unprocessed"))
            .unwrap();
        assert_eq!(
            unprocessed.get("sqs_message_id").map(String::as_str),
            Some("sqs-4")
        );
        assert_eq!(
            unprocessed.get("duration_ms").map(String::as_str),
            Some("0")
        );
        assert!(!unprocessed.contains_key("command_id"));
    }

    #[tokio::test]
    async fn captured_logs_distinguish_forbidden_receipt_and_unconfirmed_commit_without_causes() {
        let body = golden_body(1);
        for (mode, expected) in [
            (Mode::MissingTarget, "UPDATE_NOT_FOUND"),
            (Mode::Forbidden, "UPDATE_FORBIDDEN"),
            (Mode::ReceiptFailure, "RECEIPT_OPERATION_FAILED"),
            (Mode::CommitUnknown, "COMMIT_UNCONFIRMED"),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
            let use_case = FakeUseCase::new([mode]);
            let response = handler(event(&[(Some("failed"), Some(&body))]), &use_case)
                .await
                .unwrap();
            drop(guard);
            assert_eq!(failure_ids(response), ["failed"]);
            let records = events.lock().unwrap();
            let failure = records
                .iter()
                .find(|fields| fields.get("error_code").is_some_and(|v| v == expected))
                .unwrap();
            assert_eq!(
                failure.get("sqs_message_id").map(String::as_str),
                Some("failed")
            );
            assert!(!failure.contains_key("outcome"));
            assert!(failure.get("duration_ms").unwrap().parse::<u64>().is_ok());
            assert!(
                !records
                    .iter()
                    .any(|fields| fields.get("outcome").is_some_and(|v| v == "applied"))
            );
            assert!(!format!("{records:?}").contains("SENTINEL_ERROR_TEXT"));
        }
    }

    #[tokio::test]
    async fn captured_logs_drop_forged_and_unsafe_trace_values_and_never_log_payloads() {
        let mut body: serde_json::Value = serde_json::from_str(&valid_body()).unwrap();
        body["requestId"] = serde_json::json!("SENTINEL_PAYLOAD\n");
        body["correlationId"] = serde_json::json!("x".repeat(129));
        let unsafe_trace = body.to_string();
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
        let use_case = FakeUseCase::new([Mode::Complete]);
        let response = handler(event(&[(Some("sqs-ok"), Some(&unsafe_trace))]), &use_case)
            .await
            .unwrap();
        drop(guard);
        assert!(failure_ids(response).is_empty());
        {
            let records = events.lock().unwrap();
            let completed = records
                .iter()
                .find(|fields| {
                    fields
                        .get("outcome")
                        .is_some_and(|v| v == "already_completed")
                })
                .unwrap();
            assert_eq!(completed.get("request_id").map(String::as_str), Some(""));
            assert_eq!(
                completed.get("correlation_id").map(String::as_str),
                Some("")
            );
            assert!(completed.contains_key("command_id"));
            assert!(!format!("{records:?}").contains("SENTINEL_PAYLOAD"));
            assert!(!format!("{records:?}").contains("Blue vase"));
        }

        body["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        let forged = body.to_string();
        let events = Arc::new(Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
        let no_calls = FakeUseCase::new([]);
        let response = handler(
            event(&[
                (Some("bad\nSENTINEL_SQS"), Some(&forged)),
                (Some("later"), Some(&unsafe_trace)),
            ]),
            &no_calls,
        )
        .await
        .unwrap();
        drop(guard);
        assert_eq!(failure_ids(response), ["bad\nSENTINEL_SQS", "later"]);
        assert_eq!(no_calls.calls(), 0);
        let records = events.lock().unwrap();
        let failure = records
            .iter()
            .find(|fields| {
                fields
                    .get("error_code")
                    .is_some_and(|v| v == "invalid_wire_command")
            })
            .unwrap();
        assert_eq!(
            failure.get("sqs_message_id").map(String::as_str),
            Some("invalid")
        );
        assert!(!failure.contains_key("command_id"));
        assert!(!failure.contains_key("listing_source_id"));
        assert!(failure.get("duration_ms").unwrap().parse::<u64>().is_ok());
        let retained = records
            .iter()
            .find(|fields| fields.get("outcome").is_some_and(|v| v == "unprocessed"))
            .unwrap();
        assert!(!retained.contains_key("command_id"));
        for secret in [
            "SENTINEL_PAYLOAD",
            "SENTINEL_SQS",
            "Blue vase",
            "SENTINEL_ERROR_TEXT",
        ] {
            assert!(!format!("{records:?}").contains(secret));
        }
    }

    #[tokio::test]
    async fn captured_malformed_missing_body_and_exhausted_budget_have_no_command_fields() {
        let body = valid_body();
        for (invalid, budget, expected) in [
            (
                Some("SENTINEL_MALFORMED_PAYLOAD"),
                Duration::from_secs(1),
                "invalid_wire_command",
            ),
            (None, Duration::from_secs(1), "missing_message_body"),
            (
                Some(body.as_str()),
                Duration::ZERO,
                "insufficient_invocation_budget",
            ),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
            let no_calls = FakeUseCase::new([]);
            let response = handler_with_budget(
                event(&[(Some("bad"), invalid), (Some("later"), Some(&body))]),
                &no_calls,
                budget,
                Duration::from_millis(10),
            )
            .await
            .unwrap();
            drop(guard);
            assert_eq!(failure_ids(response), ["bad", "later"]);
            assert_eq!(no_calls.calls(), 0);
            let records = events.lock().unwrap();
            let failure = records
                .iter()
                .find(|fields| fields.get("error_code").is_some_and(|v| v == expected))
                .unwrap();
            assert_eq!(
                failure.get("sqs_message_id").map(String::as_str),
                Some("bad")
            );
            assert!(failure.get("duration_ms").unwrap().parse::<u64>().is_ok());
            for key in [
                "command_id",
                "submission_id",
                "listing_source_id",
                "request_id",
                "correlation_id",
                "operation",
                "item_index",
            ] {
                assert!(!failure.contains_key(key));
            }
            let suffix = records
                .iter()
                .find(|fields| fields.get("outcome").is_some_and(|v| v == "unprocessed"))
                .unwrap();
            assert_eq!(
                suffix.get("sqs_message_id").map(String::as_str),
                Some("later")
            );
            assert!(!suffix.contains_key("command_id"));
            assert!(!format!("{records:?}").contains("SENTINEL_MALFORMED_PAYLOAD"));
        }
    }

    #[tokio::test]
    async fn captured_timeout_panic_and_unprocessed_suffix_never_claim_completion() {
        let body = valid_body();
        for (mode, expected) in [
            (Mode::Wait, "execution_timeout"),
            (Mode::Panic, "handler_panicked"),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let guard = tracing::subscriber::set_default(TraceSink(events.clone()));
            let use_case = FakeUseCase::new([mode, Mode::Complete]);
            let response = handler_with_budget(
                event(&[(Some("failed"), Some(&body)), (Some("suffix"), Some(&body))]),
                &use_case,
                Duration::from_millis(15),
                Duration::from_millis(10),
            )
            .await
            .unwrap();
            drop(guard);
            assert_eq!(failure_ids(response), ["failed", "suffix"]);
            assert_eq!(use_case.calls(), 1);
            let records = events.lock().unwrap();
            let failure = records
                .iter()
                .find(|fields| fields.get("error_code").is_some_and(|v| v == expected))
                .unwrap();
            assert_eq!(
                failure.get("command_id").map(String::as_str),
                Some(codec::decode(&body).unwrap().command_id())
            );
            assert!(failure.get("duration_ms").unwrap().parse::<u64>().is_ok());
            let suffix = records
                .iter()
                .find(|fields| fields.get("outcome").is_some_and(|v| v == "unprocessed"))
                .unwrap();
            assert_eq!(
                suffix.get("sqs_message_id").map(String::as_str),
                Some("suffix")
            );
            assert_eq!(suffix.get("duration_ms").map(String::as_str), Some("0"));
            assert!(!suffix.contains_key("command_id"));
            assert!(!format!("{records:?}").contains("private use case detail"));
            assert!(
                !records
                    .iter()
                    .any(|fields| fields.get("outcome").is_some_and(|v| v == "applied"))
            );
        }
    }

    #[tokio::test]
    async fn only_verified_decoded_commands_supply_correlation_and_specific_codes() {
        let correlation = RefCell::new(None);
        let invalid = FakeUseCase::new([]);
        assert_eq!(
            process_ingestion_job("not-json", &invalid, &correlation).await,
            JobDisposition::Failed("invalid_wire_command")
        );
        assert!(correlation.borrow().is_none());
        let mut tampered: serde_json::Value = serde_json::from_str(&valid_body()).unwrap();
        tampered["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        assert_eq!(
            process_ingestion_job(&tampered.to_string(), &invalid, &correlation).await,
            JobDisposition::Failed("invalid_wire_command")
        );
        assert!(correlation.borrow().is_none());

        let body = golden_body(1);
        let decoded = codec::decode(&body).unwrap();
        let use_case = FakeUseCase::new([Mode::MissingTarget]);
        assert_eq!(
            process_ingestion_job(&body, &use_case, &correlation).await,
            JobDisposition::Failed("UPDATE_NOT_FOUND")
        );
        let command = correlation.borrow_mut().take().unwrap();
        assert_eq!(command.command_id, decoded.command_id());
        assert_eq!(command.submission_id, decoded.submission_id());
        assert_eq!(command.operation, decoded.operation().as_str());
        assert_eq!(command.index, decoded.index());

        let use_case = FakeUseCase::new([Mode::Conflict]);
        assert_eq!(
            process_ingestion_job(&body, &use_case, &correlation).await,
            JobDisposition::Failed("FINGERPRINT_CONFLICT")
        );
        correlation.borrow_mut().take();

        let use_case = FakeUseCase::new([Mode::Wait]);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                process_ingestion_job(&body, &use_case, &correlation)
            )
            .await
            .is_err()
        );
        assert_eq!(
            correlation.borrow_mut().take().unwrap().command_id,
            decoded.command_id()
        );
    }

    #[tokio::test]
    async fn valid_envelope_and_duplicate_completion_are_the_only_acknowledgments() {
        let body = valid_body();
        let use_case = FakeUseCase::new([Mode::Applied, Mode::Complete]);
        let response = handler(
            event(&[(Some("sqs-1"), Some(&body)), (Some("sqs-2"), Some(&body))]),
            &use_case,
        )
        .await
        .unwrap();
        assert!(failure_ids(response).is_empty());
        assert_eq!(use_case.calls(), 2);
        let expected = codec::decode(&body).unwrap().command_id().to_owned();
        assert_eq!(
            *use_case.command_ids.lock().unwrap(),
            [expected.clone(), expected]
        );
    }

    #[tokio::test]
    async fn failure_blocks_every_successor_even_from_another_fifo_group() {
        let body = valid_body();
        let other_group = golden_body(4);
        assert_ne!(
            codec::fifo_group_id(&codec::decode(&body).unwrap()).unwrap(),
            codec::fifo_group_id(&codec::decode(&other_group).unwrap()).unwrap(),
        );
        let use_case = FakeUseCase::new([Mode::Complete, Mode::Failure, Mode::Complete]);
        let response = handler(
            event(&[
                (Some("one"), Some(&body)),
                (Some("two"), Some(&body)),
                (Some("three"), Some(&other_group)),
            ]),
            &use_case,
        )
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["two", "three"]);
        assert_eq!(use_case.calls(), 2);
    }

    #[tokio::test]
    async fn first_and_last_failures_retain_only_the_incomplete_suffix() {
        let body = valid_body();
        let records = [
            (Some("first"), Some(body.as_str())),
            (Some("middle"), Some(body.as_str())),
            (Some("last"), Some(body.as_str())),
        ];
        let first_failure = FakeUseCase::new([Mode::Failure]);
        assert_eq!(
            failure_ids(handler(event(&records), &first_failure).await.unwrap()),
            ["first", "middle", "last"]
        );
        assert_eq!(first_failure.calls(), 1);

        let last_failure = FakeUseCase::new([Mode::Applied, Mode::Complete, Mode::Failure]);
        assert_eq!(
            failure_ids(handler(event(&records), &last_failure).await.unwrap()),
            ["last"]
        );
        assert_eq!(last_failure.calls(), 3);
    }

    #[tokio::test]
    async fn replayed_receipt_then_distinct_command_can_complete_after_lost_response() {
        let old = golden_body(0);
        let new = golden_body(1);
        assert_ne!(
            codec::decode(&old).unwrap().command_id(),
            codec::decode(&new).unwrap().command_id()
        );
        // The previous invocation committed `old` but lost its response. The service
        // recognizes its receipt on redelivery before a new command is attempted.
        let use_case = FakeUseCase::new([Mode::Complete, Mode::Applied]);
        let response = handler(
            event(&[(Some("redelivered"), Some(&old)), (Some("new"), Some(&new))]),
            &use_case,
        )
        .await
        .unwrap();
        assert!(failure_ids(response).is_empty());
        assert_eq!(use_case.calls(), 2);
        assert_eq!(
            *use_case.command_ids.lock().unwrap(),
            [
                codec::decode(&old).unwrap().command_id().to_owned(),
                codec::decode(&new).unwrap().command_id().to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn unconfirmed_commit_blocks_successors_even_if_the_commit_may_have_succeeded() {
        let body = valid_body();
        let use_case = FakeUseCase::new([Mode::CommitUnknown, Mode::Complete]);
        let response = handler(
            event(&[(Some("unknown"), Some(&body)), (Some("later"), Some(&body))]),
            &use_case,
        )
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["unknown", "later"]);
        assert_eq!(use_case.calls(), 1);
    }

    #[tokio::test]
    async fn malformed_wire_and_missing_body_stop_before_any_service_call() {
        let body = valid_body();
        for invalid in [Some("not-json"), Some("{}"), None] {
            let use_case = FakeUseCase::new([Mode::Complete]);
            let response = handler(
                event(&[(Some("bad"), invalid), (Some("later"), Some(&body))]),
                &use_case,
            )
            .await
            .unwrap();
            assert_eq!(failure_ids(response), ["bad", "later"]);
            assert_eq!(use_case.calls(), 0);
        }
        let mut tampered: serde_json::Value = serde_json::from_str(&body).unwrap();
        tampered["semanticFingerprint"] = serde_json::json!("0".repeat(64));
        let use_case = FakeUseCase::new([Mode::Complete]);
        let response = handler(
            event(&[(Some("bad"), Some(&tampered.to_string()))]),
            &use_case,
        )
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["bad"]);
        assert_eq!(use_case.calls(), 0);
    }

    #[tokio::test]
    async fn valid_prefix_then_malformed_record_stops_before_the_remaining_suffix() {
        let body = valid_body();
        let use_case = FakeUseCase::new([Mode::Complete, Mode::Complete]);
        let response = handler(
            event(&[
                (Some("complete"), Some(&body)),
                (Some("malformed"), Some("not-json")),
                (Some("unprocessed"), Some(&body)),
            ]),
            &use_case,
        )
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["malformed", "unprocessed"]);
        assert_eq!(use_case.calls(), 1);
    }

    #[tokio::test]
    async fn a_full_ten_record_fifo_batch_can_complete_in_order() {
        let body = valid_body();
        let ids: Vec<String> = (0..10).map(|i| format!("sqs-{i}")).collect();
        let records: Vec<_> = ids
            .iter()
            .map(|id| (Some(id.as_str()), Some(body.as_str())))
            .collect();
        let use_case = FakeUseCase::new([Mode::Complete; 10]);
        let response = handler(event(&records), &use_case).await.unwrap();
        assert!(failure_ids(response).is_empty());
        assert_eq!(use_case.calls(), 10);
    }

    #[tokio::test]
    async fn timeout_panic_and_no_budget_never_start_a_successor() {
        let body = valid_body();
        for mode in [Mode::Wait, Mode::Panic] {
            let use_case = FakeUseCase::new([mode, Mode::Complete]);
            let response = handler_with_budget(
                event(&[(Some("bad"), Some(&body)), (Some("later"), Some(&body))]),
                &use_case,
                Duration::from_millis(15),
                Duration::from_millis(10),
            )
            .await
            .unwrap();
            assert_eq!(failure_ids(response), ["bad", "later"]);
            assert_eq!(use_case.calls(), 1);
        }
        let use_case = FakeUseCase::new([Mode::Complete]);
        let response = handler_with_budget(
            event(&[(Some("bad"), Some(&body)), (Some("later"), Some(&body))]),
            &use_case,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["bad", "later"]);
        assert_eq!(use_case.calls(), 0);
    }

    #[tokio::test]
    async fn missing_blank_and_duplicate_ids_fail_invocation_before_setup() {
        let body = valid_body();
        for records in [
            [
                (Some("first"), Some(body.as_str())),
                (None, Some(body.as_str())),
            ],
            [
                (Some("first"), Some(body.as_str())),
                (Some("  "), Some(body.as_str())),
            ],
            [
                (Some("first"), Some(body.as_str())),
                (Some("first"), Some(body.as_str())),
            ],
        ] {
            let calls = AtomicUsize::new(0);
            assert!(
                handle_invocation(event(&records), || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err(Error::from("should not be called")) }
                })
                .await
                .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn empty_and_expired_invocations_do_not_start_setup() {
        let calls = AtomicUsize::new(0);
        let empty = handle_invocation(event(&[]), || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(Error::from("must not run")) }
        })
        .await
        .unwrap();
        assert!(failure_ids(empty).is_empty());

        let body = valid_body();
        let mut expired = event(&[(Some("first"), Some(&body))]);
        expired.context.deadline = 0;
        let response = handle_invocation(expired, || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(Error::from("must not run")) }
        })
        .await
        .unwrap();
        assert_eq!(failure_ids(response), ["first"]);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn setup_failure_timeout_and_panic_retain_whole_batch() {
        let body = valid_body();
        for mode in ["error", "wait", "panic", "construction_panic"] {
            let mut input = event(&[(Some("first"), Some(&body)), (Some("second"), Some(&body))]);
            input.context.deadline = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
                + 5_030;
            let response = handle_invocation(input, move || {
                if mode == "construction_panic" {
                    panic!("private setup construction detail");
                }
                async move {
                    match mode {
                        "error" => Err(Error::from("private setup failure")),
                        "wait" => std::future::pending().await,
                        _ => panic!("private setup panic"),
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(failure_ids(response), ["first", "second"]);
        }
    }

    #[tokio::test]
    async fn successful_setup_dispatches_records_through_the_use_case() {
        let body = valid_body();
        let use_case = Arc::new(FakeUseCase::new([Mode::Complete, Mode::Complete]));
        let response = handle_invocation(
            event(&[(Some("first"), Some(&body)), (Some("second"), Some(&body))]),
            || async {
                let use_case: Arc<dyn ProcessProductListingIngestionUseCase> = use_case.clone();
                Ok(use_case)
            },
        )
        .await
        .unwrap();
        assert!(failure_ids(response).is_empty());
        assert_eq!(use_case.calls(), 2);
    }
}
