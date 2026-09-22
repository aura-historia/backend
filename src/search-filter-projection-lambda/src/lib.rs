use aura_historia_worker::search_filter_projection::{
    SearchFilterProjectionJobDisposition, process_search_filter_projection_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use lambda_runtime::{Error, LambdaEvent};
use opensearch::OpenSearch;
use platform_lambda_bootstrap::LambdaInvocationBudget;
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::SqlxSearchFilterIndexReader;
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
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

/// Uses a single deadline for credential refresh, composition, record work, and response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

/// Production composition for the authoritative PostgreSQL-to-OpenSearch saved-filter projection.
pub fn compose_projection_use_case(
    pool: sqlx::PgPool,
    open_search: OpenSearch,
) -> Arc<dyn ProjectSearchFilterChangeUseCase> {
    Arc::new(ProjectSearchFilterChangeHandler::new(
        SqlxSearchFilterIndexReader::new(pool),
        OpenSearchSearchFilterIndex::new(open_search),
    ))
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

/// Uses the budget created at the invocation edge so setup and record work share one deadline.
pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
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

/// Retains every record when startup cannot establish a safe authoritative composition.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = event
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
    Ok(response)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
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
            warn!(message_id = %message_id, outcome = "insufficient_invocation_budget", "Saved-filter projection record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
            continue;
        }
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => SearchFilterProjectionJobDisposition::Poison("missing_message_body"),
        };
        if !matches!(
            disposition,
            SearchFilterProjectionJobDisposition::Complete(_)
        ) {
            warn!(message_id = %message_id, outcome = disposition.category(), "Saved-filter projection record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished saved-filter OpenSearch projection batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
    processing_budget: Duration,
) -> SearchFilterProjectionJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_search_filter_projection_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => {
            SearchFilterProjectionJobDisposition::DependencyUnavailable("execution_timeout")
        }
        Err(_) => SearchFilterProjectionJobDisposition::DependencyUnavailable("handler_panicked"),
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
    use lambda_runtime::Context;
    use search_filter_service::{
        ports::SearchFilterProjectionWriteOutcome,
        use_cases::{
            ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeError,
            ProjectSearchFilterChangeResult,
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

    #[tokio::test]
    async fn acknowledges_applied_and_stale_fences_but_retains_failures() {
        let use_case = FakeUseCase::new([
            FakeResult::Applied,
            FakeResult::Stale,
            FakeResult::Unavailable,
            FakeResult::Invalid,
        ]);
        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("unavailable"), valid_body()),
                (Some("invalid"), valid_body()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(failure_ids(response), ["unavailable", "invalid"]);
        assert_eq!(use_case.calls(), 4);
    }

    #[tokio::test]
    async fn retains_malformed_timeout_panic_and_unstarted_records() {
        let malformed = handler(
            events([(Some("poison"), "not-json".to_owned())]),
            &FakeUseCase::new([]),
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(malformed), ["poison"]);

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

        let use_case = FakeUseCase::new([FakeResult::Pending, FakeResult::Applied]);
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
    fn retains_every_record_when_composition_fails() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial batch response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    fn valid_body() -> String {
        r#"{"schema_version":2,"scope":"search-filter-projection","job_type":"SEARCH_FILTER_CHANGED","idempotency_key":"search-filter:sf_01h455vb4pex5vy7enb1p677vn:1:insert","ordering_key":"search-filter:sf_01h455vb4pex5vy7enb1p677vn","payload":{"user_id":"usr_01h455vb4pex5vy7enb1p677vn","user_search_filter_id":"sf_01h455vb4pex5vy7enb1p677vn","version":1,"operation":"INSERT"}}"#.to_owned()
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
        Applied,
        Stale,
        Unavailable,
        Invalid,
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
    impl ProjectSearchFilterChangeUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: ProjectSearchFilterChangeCommand,
        ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let next = self.results.lock().expect("results lock").pop_front();
            match next {
                Some(FakeResult::Applied) => Ok(ProjectSearchFilterChangeResult {
                    outcome: SearchFilterProjectionWriteOutcome::Applied,
                }),
                Some(FakeResult::Stale) => Ok(ProjectSearchFilterChangeResult {
                    outcome: SearchFilterProjectionWriteOutcome::Stale,
                }),
                Some(FakeResult::Unavailable) => Err(ProjectSearchFilterChangeError::ReadFailed {
                    source: Box::new(std::io::Error::other("unavailable")),
                }),
                Some(FakeResult::Invalid) => {
                    Err(ProjectSearchFilterChangeError::InvalidSourceVersion)
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                Some(FakeResult::Panic) => panic!("test panic"),
                None => Ok(ProjectSearchFilterChangeResult {
                    outcome: SearchFilterProjectionWriteOutcome::Applied,
                }),
            }
        }
    }
}
