use aura_historia_jobs::{DomainJob, DomainJobPayload, SearchFilterOperation, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use opensearch::OpenSearch;
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::SqlxSearchFilterIndexReader;
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeError,
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
    SearchFilterProjectionOperation,
};
use std::{sync::Arc, time::Duration};
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
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_search_filter_projection_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(
                    disposition,
                    SearchFilterProjectionJobDisposition::Complete(_)
                ),
                disposition.category(),
            ),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
                "Saved-filter projection record retained for SQS retry or redrive");
        }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished saved-filter OpenSearch projection batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterProjectionJobDisposition {
    Complete(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterProjectionJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_search_filter_projection_job(
    body: &str,
    use_case: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
) -> SearchFilterProjectionJobDisposition {
    match decode::<SearchFilterOperation>(body, WorkerScope::SearchFilterProjection) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterProjectionJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O: Into<SearchFilterOperation>>(
    handler: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
    job: DomainJob<O>,
) -> SearchFilterProjectionJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return SearchFilterProjectionJobDisposition::Poison("projection_metadata_invalid");
    };
    match handler.execute(command).await {
        Ok(result) => {
            tracing::info!(outcome = ?result.outcome, "search filter projection write completed");
            SearchFilterProjectionJobDisposition::Complete("projection_written_or_stale")
        }
        Err(
            ProjectSearchFilterChangeError::InvalidSourceVersion
            | ProjectSearchFilterChangeError::DeleteVersionOverflow
            | ProjectSearchFilterChangeError::InvalidPersistedState { .. },
        ) => SearchFilterProjectionJobDisposition::Poison("projection_state_invalid"),
        Err(
            ProjectSearchFilterChangeError::ReadFailed { .. }
            | ProjectSearchFilterChangeError::WriteFailed { .. },
        ) => SearchFilterProjectionJobDisposition::DependencyUnavailable("projection_unavailable"),
    }
}

pub fn command_from_job<O: Into<SearchFilterOperation>>(
    job: DomainJob<O>,
) -> Result<ProjectSearchFilterChangeCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterChanged(change) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    if change.version <= 0 {
        return Err(aura_historia_jobs::InvalidJob);
    }
    Ok(ProjectSearchFilterChangeCommand {
        search_filter_id: change.user_search_filter_id,
        source_version: change.version,
        operation: match change.operation.into() {
            SearchFilterOperation::Insert | SearchFilterOperation::Update => {
                SearchFilterProjectionOperation::Upsert
            }
            SearchFilterOperation::Delete => SearchFilterProjectionOperation::Delete,
        },
    })
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
