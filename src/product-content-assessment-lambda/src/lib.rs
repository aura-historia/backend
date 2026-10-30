use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::{FailureDiagnostic, LambdaInvocationBudget};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSourceReader,
    SqlxProductListingContentAssessmentWriterFactory,
};
use product_listing_service::use_cases::{
    AssessProductListingContentCommand, AssessProductListingContentEventError,
    AssessProductListingContentEventHandler, AssessProductListingContentEventOutcome,
    AssessProductListingContentEventUseCase,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the existing PostgreSQL-only assessment use case for the native SQS Lambda.
pub fn compose_product_content_assessment_use_case(
    pool: sqlx::PgPool,
) -> Arc<dyn AssessProductListingContentEventUseCase> {
    Arc::new(AssessProductListingContentEventHandler::new(
        SqlxProductListingContentAssessmentSourceReader::new(pool.clone()),
        SqlxUnitOfWork::new(pool),
        SqlxProductListingContentAssessmentWriterFactory::new(),
    ))
}

/// Computes one deadline shared by credential refresh, composition, record work, and response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
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

/// Retains every record when credential refresh or composition cannot finish safely.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_product_content_assessment_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let started_at = results.started_at;
    let response = results.finish(|attempt| {
        let (complete, category, failure) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(
                    disposition,
                    ProductContentAssessmentJobDisposition::Complete(_)
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
                http_status = failure.and_then(FailureDiagnostic::http_status),
                sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
                "Product content-assessment record retained for SQS retry or redrive");
        }
        complete
    });
    info!(
        assessment_sqs_message_count = record_count,
        assessment_failed_sqs_message_count = response.batch_item_failures.len(),
        assessment_batch_duration_ms = started_at.elapsed().as_millis(),
        "Finished ProductListing content-assessment batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductContentAssessmentJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl ProductContentAssessmentJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "assessment_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_product_content_assessment_job(
    body: &str,
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
) -> ProductContentAssessmentJobDisposition {
    match decode(body, WorkerScope::ProductListingContentAssessment) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductContentAssessmentJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
    job: DomainJob,
) -> ProductContentAssessmentJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductContentAssessmentJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-content-assessment:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => assessment_disposition(result.outcome),
        Err(error) => assessment_error_disposition(&error),
    }
}

/// A provider is intentionally not part of this scope. Source, write, commit, and unknown
/// completion errors must remain retryable rather than being acknowledged. Only the failed stage
/// and safe cause facts are kept; the error chain itself is never logged.
fn assessment_error_disposition(
    error: &AssessProductListingContentEventError,
) -> ProductContentAssessmentJobDisposition {
    use AssessProductListingContentEventError as E;
    use ProductContentAssessmentJobDisposition as D;
    let stage = match error {
        E::ServiceOrSystemPrincipalRequired => return D::Poison("system_principal_required"),
        E::SourceReadFailed { .. } => "source_read",
        E::BeginTransactionFailed { .. } => "write_begin",
        E::AssessmentWriteFailed { .. } => "assessment_write",
        E::CommitTransactionFailed { .. } => "write_commit",
    };
    D::DependencyUnavailable(FailureDiagnostic::postgres(stage, error))
}

pub fn assessment_disposition(
    outcome: AssessProductListingContentEventOutcome,
) -> ProductContentAssessmentJobDisposition {
    use AssessProductListingContentEventOutcome as O;
    match outcome {
        O::Applied => ProductContentAssessmentJobDisposition::Complete("applied"),
        O::Cleared => ProductContentAssessmentJobDisposition::Complete("cleared"),
        O::Duplicate => ProductContentAssessmentJobDisposition::Complete("duplicate"),
        O::Stale => ProductContentAssessmentJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductContentAssessmentJobDisposition::Complete("ignored_event"),
        O::ProductListingNotFound => {
            ProductContentAssessmentJobDisposition::Retry("missing_source")
        }
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<AssessProductListingContentCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(AssessProductListingContentCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::use_cases::{
        AssessProductListingContentCommand, AssessProductListingContentEventError,
        AssessProductListingContentEventOutcome, AssessProductListingContentEventResult,
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
    async fn acknowledges_committed_guarded_outcomes_and_retries_all_unfinished_records() {
        let use_case = FakeUseCase::new([
            FakeResult::Outcome(AssessProductListingContentEventOutcome::Applied),
            FakeResult::Outcome(AssessProductListingContentEventOutcome::Cleared),
            FakeResult::Outcome(AssessProductListingContentEventOutcome::Duplicate),
            FakeResult::Outcome(AssessProductListingContentEventOutcome::Stale),
            FakeResult::Outcome(AssessProductListingContentEventOutcome::IgnoredEvent),
            FakeResult::Outcome(AssessProductListingContentEventOutcome::ProductListingNotFound),
            FakeResult::Unavailable,
        ]);

        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("cleared"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("ignored"), valid_body()),
                (Some("missing-source"), valid_body()),
                (Some("unavailable"), valid_body()),
                (Some("poison"), "not-json".to_owned()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(
            failure_ids(response),
            ["missing-source", "unavailable", "poison"]
        );
        assert_eq!(use_case.calls(), 7);
    }

    #[tokio::test]
    async fn retains_records_when_the_shared_or_per_record_budget_is_exhausted() {
        let unstarted = FakeUseCase::new([FakeResult::Outcome(
            AssessProductListingContentEventOutcome::Applied,
        )]);
        let response = handler_with_budget(
            events([(Some("unstarted"), valid_body())]),
            &unstarted,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(response), ["unstarted"]);
        assert_eq!(unstarted.calls(), 0);

        let timed_out = FakeUseCase::new([FakeResult::Pending]);
        let response = handler_with_budget(
            events([(Some("timed-out"), valid_body())]),
            &timed_out,
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .await
        .expect("handler response");
        assert_eq!(failure_ids(response), ["timed-out"]);
        assert_eq!(timed_out.calls(), 1);
    }

    #[test]
    fn retains_every_record_when_postgres_composition_cannot_complete() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial batch response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let use_case = FakeUseCase::new([FakeResult::Unavailable]);
        let (logs, guard) = CapturedLogs::start();

        let response = handler(events([(Some("unavailable"), valid_body())]), &use_case)
            .await
            .expect("handler response");
        drop(guard);

        assert_eq!(failure_ids(response), ["unavailable"]);
        let retained = logs.events_where(
            "message",
            "Product content-assessment record retained for SQS retry or redrive",
        );
        assert_eq!(1, retained.len());
        assert_eq!(Some("source_read"), retained[0]["stage"].as_str());
        assert_eq!(Some("postgres"), retained[0]["dependency"].as_str());
        assert_eq!(Some("io"), retained[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"product-content-assessment","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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

    enum FakeResult {
        Outcome(AssessProductListingContentEventOutcome),
        Unavailable,
        Pending,
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
    impl AssessProductListingContentEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _context: &application::operation_context::OperationContext,
            _command: AssessProductListingContentCommand,
        ) -> Result<AssessProductListingContentEventResult, AssessProductListingContentEventError>
        {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = { self.results.lock().expect("results lock").pop_front() };
            match result {
                Some(FakeResult::Outcome(outcome)) => {
                    Ok(AssessProductListingContentEventResult { outcome })
                }
                Some(FakeResult::Unavailable) => {
                    Err(AssessProductListingContentEventError::SourceReadFailed {
                        source: Box::new(sqlx::Error::Io(std::io::Error::other(SENTINEL))),
                    })
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
