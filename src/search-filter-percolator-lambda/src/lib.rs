use aura_historia_worker::search_filter_percolator::{
    SearchFilterPercolatorJobDisposition, process_search_filter_percolator_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent};
use large_language_model::VertexAiGemini;
use opensearch::OpenSearch;
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingCurrentEventGuardFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterMatchWriterFactory,
};
use search_filter_service::use_cases::{
    MatchProductListingEventHandler, MatchProductListingEventUseCase,
};
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the single source-guarded, PostgreSQL-backed match use case used by the Lambda.
pub fn compose_percolator_use_case(
    pool: sqlx::PgPool,
    open_search: OpenSearch,
    evaluator: VertexAiGemini,
) -> Arc<dyn MatchProductListingEventUseCase> {
    Arc::new(MatchProductListingEventHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxProductListingCurrentEventGuardFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchSearchFilterIndex::new(open_search),
        evaluator,
        SqlxActiveSearchFilterMatchCandidateReaderFactory,
        SqlxSearchFilterMatchWriterFactory,
    ))
}

/// Computes the usable Lambda deadline, reserving time for a truthful SQS response.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
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

/// Retains every valid record when credential refresh or composition cannot finish safely.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    let failures = event
        .payload
        .records
        .iter()
        .map(|record| {
            record.message_id.clone().ok_or_else(|| {
                Error::from("SQS event record has no message ID; fail whole invocation")
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(batch_failure)
        .collect();
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let record_count = event.payload.records.len();
    let invocation_started_at = Instant::now();
    let mut failures = Vec::new();

    for record in event.payload.records {
        let message_id = record.message_id.ok_or_else(|| {
            Error::from("SQS event record has no message ID; fail whole invocation")
        })?;
        let remaining = invocation_budget.saturating_sub(invocation_started_at.elapsed());
        let processing_budget = max_record_processing_budget.min(remaining);
        if processing_budget.is_zero() {
            warn!(
                message_id = %message_id,
                outcome = "insufficient_invocation_budget",
                "Saved-filter percolator record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
            continue;
        }
        let processing_started_at = Instant::now();
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => SearchFilterPercolatorJobDisposition::Poison("missing_message_body"),
        };
        let processing_duration_ms = processing_started_at.elapsed().as_millis();
        if !matches!(
            disposition,
            SearchFilterPercolatorJobDisposition::Complete(_)
        ) {
            warn!(
                message_id = %message_id,
                outcome = disposition.category(),
                processing_duration_ms,
                "Saved-filter percolator record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
        } else {
            info!(
                message_id = %message_id,
                outcome = disposition.category(),
                processing_duration_ms,
                "Saved-filter percolator record completed"
            );
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished saved-filter percolator batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
    processing_budget: Duration,
) -> SearchFilterPercolatorJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_search_filter_percolator_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => SearchFilterPercolatorJobDisposition::Retry("execution_timeout"),
        Err(_) => SearchFilterPercolatorJobDisposition::Retry("handler_panicked"),
    }
}

fn batch_failure(message_id: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = message_id;
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use domain_primitives::event_id::EventId;
    use lambda_runtime::Context;
    use product_listing_core::product_listing_id::ProductListingId;
    use search_filter_service::use_cases::{
        MatchProductListingEventCommand, MatchProductListingEventError,
        MatchProductListingEventOutcome, MatchProductListingEventResult,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    #[tokio::test]
    async fn acknowledges_only_durably_complete_percolation_outcomes() {
        let use_case = FakeUseCase::new([
            FakeResult::Outcome(MatchProductListingEventOutcome::Processed),
            FakeResult::Outcome(MatchProductListingEventOutcome::DuplicateAlreadyPersisted),
            FakeResult::Outcome(MatchProductListingEventOutcome::StaleSourceSkipped),
            FakeResult::Outcome(MatchProductListingEventOutcome::InactiveSourceSkipped),
            FakeResult::Outcome(MatchProductListingEventOutcome::IgnoredEventType),
            FakeResult::Outcome(MatchProductListingEventOutcome::SourceNotFound),
        ]);

        let response = handler(
            events([
                (Some("processed"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("inactive"), valid_body()),
                (Some("ignored"), valid_body()),
                (Some("missing"), valid_body()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(failure_ids(response), ["missing"]);
        assert_eq!(use_case.calls(), 6);
    }

    #[tokio::test]
    async fn retains_malformed_dependency_timeout_and_panic_records() {
        let use_case = FakeUseCase::new([
            FakeResult::StateInvalid,
            FakeResult::Pending,
            FakeResult::Panic,
        ]);
        let response = handler_with_budget(
            events([
                (Some("malformed"), "not-json".to_owned()),
                (Some("state-invalid"), valid_body()),
                (Some("timeout"), valid_body()),
                (Some("panic"), valid_body()),
            ]),
            &use_case,
            Duration::from_millis(10),
            Duration::from_millis(1),
        )
        .await
        .expect("handler response");

        assert_eq!(
            failure_ids(response),
            ["malformed", "state-invalid", "timeout", "panic"]
        );
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
        r#"{"schema_version":2,"scope":"search-filter-percolator","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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
            .expect("current time")
            .as_millis()
            .try_into()
            .expect("epoch milliseconds fit in u64")
    }

    enum FakeResult {
        Outcome(MatchProductListingEventOutcome),
        StateInvalid,
        Pending,
        Panic,
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
    impl MatchProductListingEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: MatchProductListingEventCommand,
        ) -> Result<MatchProductListingEventResult, MatchProductListingEventError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = self.results.lock().expect("results lock").pop_front();
            match result {
                Some(FakeResult::Outcome(outcome)) => Ok(MatchProductListingEventResult {
                    outcome,
                    percolated_count: 0,
                    persisted_match_count: 0,
                    enhanced_evaluation_failure_count: 0,
                }),
                Some(FakeResult::StateInvalid) => {
                    Err(MatchProductListingEventError::ProductListingSourceMismatch)
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                Some(FakeResult::Panic) => panic!("fake percolator panic"),
                None => panic!("unexpected use case invocation"),
            }
        }
    }

    #[test]
    fn valid_job_identifiers_are_typed_values() {
        assert!(EventId::try_from("evt_01h455vb4pex5vy7enb1p677vn").is_ok());
        assert!(ProductListingId::try_from("pl_01h455vb4pex5vy7enb1p677vn").is_ok());
    }
}
