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
    ProductListingIngestionCompletion, ProductListingIngestionError,
};
use std::{cell::Cell, future::Future, sync::Arc, time::Duration};
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
    if !setup_ready.get() && response.is_ok() {
        for _ in 0..record_count {
            log_unprocessed();
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
    let response = process_fifo_batch(
        event,
        remaining,
        per_record_cap,
        |body| async move { process_ingestion_job(&body, use_case).await },
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
            if complete {
                IngestionMetricOutcome::Completed.emit();
                info!(outcome = category, "Ingestion command completed");
            } else {
                IngestionMetricOutcome::Failed.emit();
                warn!(outcome = category, "Ingestion command retained for retry");
            }
            complete
        },
    )
    .await?;
    for _ in attempted..record_count {
        log_unprocessed();
    }
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished ProductListing ingestion batch"
    );
    Ok(response)
}

fn log_unprocessed() {
    IngestionMetricOutcome::Unprocessed.emit();
    info!("Ingestion record retained without execution");
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
) -> JobDisposition {
    let envelope =
        match codec::decode(body).and_then(codec::IngestionEnvelopeV1::into_service_envelope) {
            Ok(envelope) => envelope,
            Err(_) => return JobDisposition::Failed("invalid_wire_command"),
        };
    match use_case.execute(envelope).await {
        Ok(ProductListingIngestionCompletion::Applied(_)) => JobDisposition::Complete("applied"),
        Ok(ProductListingIngestionCompletion::AlreadyCompleted) => {
            JobDisposition::Complete("already_completed")
        }
        Err(ProductListingIngestionError::CommitTransactionFailed) => {
            JobDisposition::Failed("commit_unconfirmed")
        }
        Err(
            ProductListingIngestionError::FingerprintConflict
            | ProductListingIngestionError::InvalidMetadata,
        ) => JobDisposition::Failed("invalid_command_state"),
        Err(_) => JobDisposition::Failed("use_case_failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::{
        ports::ProductListingRawStreamId,
        use_cases::{
            CaptureProductListingRawObservationResult, ProductListingIngestionEffect,
            ProductListingIngestionEnvelope,
        },
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    const GOLDEN: &str =
        include_str!("../../product-listing-ingestion-sqs/tests/fixtures/ingestion_v1.json");

    #[derive(Clone, Copy)]
    enum Mode {
        Applied,
        Complete,
        Failure,
        CommitUnknown,
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
