use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent};
use large_language_model::VertexAiGemini;
use opensearch::OpenSearch;
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
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
    MatchProductListingEventCommand, MatchProductListingEventError,
    MatchProductListingEventHandler, MatchProductListingEventOutcome,
    MatchProductListingEventUseCase,
};
use std::{sync::Arc, time::Duration};
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
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_search_filter_percolator_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (matches!(disposition, SearchFilterPercolatorJobDisposition::Complete(_)), disposition.category()),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
        processing_duration_ms = attempt.duration.as_millis(),
                "Saved-filter percolator record retained for SQS retry or redrive");
        }
        if complete { info!(message_id = %attempt.message_id, outcome = category, processing_duration_ms = attempt.duration.as_millis(), "Saved-filter percolator record completed"); }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished saved-filter percolator batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterPercolatorJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterPercolatorJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_search_filter_percolator_job(
    body: &str,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
) -> SearchFilterPercolatorJobDisposition {
    match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::SearchFilterPercolator,
    ) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterPercolatorJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O>(
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
    job: DomainJob<O>,
) -> SearchFilterPercolatorJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return SearchFilterPercolatorJobDisposition::Poison("unexpected_payload");
    };
    match use_case
        .execute(MatchProductListingEventCommand {
            origin_event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => {
            tracing::info!(
                percolated_count = result.percolated_count,
                persisted_match_count = result.persisted_match_count,
                enhanced_evaluation_failure_count = result.enhanced_evaluation_failure_count,
                outcome = percolator_outcome(result.outcome).category(),
                "percolation completed"
            );
            percolator_outcome(result.outcome)
        }
        Err(error) => {
            use MatchProductListingEventError as E;
            match error {
                E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::SaleSnapshotStateInvalid { .. }
                | E::EventSnapshotStateInvalid { .. }
                | E::EventValuationConversionFailed { .. }
                | E::CandidateStateInvalid { .. }
                | E::PersistedMatchStateInvalid { .. } => {
                    SearchFilterPercolatorJobDisposition::Poison("percolator_state_invalid")
                }
                E::SaleSnapshotNotFound { .. } | E::EventSnapshotNotFound { .. } => {
                    SearchFilterPercolatorJobDisposition::Retry("valuation_snapshot_missing")
                }
                _ => SearchFilterPercolatorJobDisposition::DependencyUnavailable(
                    "percolator_unavailable",
                ),
            }
        }
    }
}

pub fn percolator_outcome(
    outcome: MatchProductListingEventOutcome,
) -> SearchFilterPercolatorJobDisposition {
    match outcome {
        MatchProductListingEventOutcome::Processed => {
            SearchFilterPercolatorJobDisposition::Complete("processed")
        }
        MatchProductListingEventOutcome::DuplicateAlreadyPersisted => {
            SearchFilterPercolatorJobDisposition::Complete("duplicate")
        }
        MatchProductListingEventOutcome::StaleSourceSkipped => {
            SearchFilterPercolatorJobDisposition::Complete("stale")
        }
        MatchProductListingEventOutcome::InactiveSourceSkipped => {
            SearchFilterPercolatorJobDisposition::Complete("inactive_source")
        }
        MatchProductListingEventOutcome::IgnoredEventType => {
            SearchFilterPercolatorJobDisposition::Complete("ignored_event")
        }
        MatchProductListingEventOutcome::SourceNotFound => {
            SearchFilterPercolatorJobDisposition::Retry("missing_source")
        }
    }
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
