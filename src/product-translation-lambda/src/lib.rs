use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use large_language_model::{AnthropicClaude, LargeLanguageModel, LargeLanguageModelError};
use platform_lambda_bootstrap::{FailureDiagnostic, LambdaInvocationBudget, UNCLASSIFIED_FAILURE};
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingTranslationSourceReader, SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::ports::ProductListingTitleTranslationError;
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventError,
    TranslateProductListingEventHandler, TranslateProductListingEventOutcome,
    TranslateProductListingEventUseCase,
};
use product_listing_translation_llm::LargeLanguageModelProductListingTitleTranslator;
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

// The translator has a bounded 30-second provider request. The remaining envelope covers source
// reads, guarded persistence, credential refresh, and returning the partial SQS failure response.
const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the existing PostgreSQL-backed, first-committed-completion translation use case.
///
/// The source read completes before inference and the guarded write transaction starts only after
/// it, so a reusable pool connection is not retained while Claude is awaited.
pub fn compose_product_translation_use_case(
    pool: sqlx::PgPool,
    translator: AnthropicClaude,
) -> Arc<dyn TranslateProductListingEventUseCase> {
    compose_product_translation_use_case_with_model(pool, translator)
}

pub fn compose_product_translation_use_case_with_model<L>(
    pool: sqlx::PgPool,
    model: L,
) -> Arc<dyn TranslateProductListingEventUseCase>
where
    L: LargeLanguageModel + 'static,
{
    Arc::new(TranslateProductListingEventHandler::new(
        SqlxProductListingTranslationSourceReader::new(pool.clone()),
        LargeLanguageModelProductListingTitleTranslator::new(model),
        SqlxUnitOfWork::new(pool),
        SqlxProductListingTranslationWriterFactory::new(),
    ))
}

/// Computes the usable Lambda deadline, including provider setup and SQS response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
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

/// Retains every source record when credentials or PostgreSQL composition cannot become ready.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_product_translation_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let started_at = results.started_at;
    let mut guard_rejection_count = 0_usize;
    let mut retry_count = 0_usize;
    let response = results.finish(|attempt| {
        let (complete, category, failure) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(disposition, ProductTranslationJobDisposition::Complete(_)),
                disposition.category(),
                disposition.failure_diagnostic(),
            ),
            RecordOutcome::MissingBody => (false, "missing_message_body", None),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget", None),
            RecordOutcome::TimedOut => (false, "execution_timeout", None),
            RecordOutcome::Panicked => (false, "handler_panicked", None),
        };
        if is_guard_rejection(&attempt.outcome) {
            guard_rejection_count += 1;
        }
        if complete {
            info!(
                message_id = %attempt.message_id,
                outcome = category,
                processing_duration_ms = attempt.duration.as_millis(),
                "Product translation record completed"
            );
        } else {
            retry_count += 1;
            warn!(
                message_id = %attempt.message_id,
                outcome = category,
                stage = failure.map(FailureDiagnostic::stage),
                dependency = failure.and_then(FailureDiagnostic::dependency),
                error_category = failure.map(FailureDiagnostic::category),
                http_status = failure.and_then(FailureDiagnostic::http_status),
                sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
                processing_duration_ms = attempt.duration.as_millis(),
                "Product translation record retained for SQS retry or redrive"
            );
        }
        complete
    });
    info!(
        translation_sqs_message_count = record_count,
        translation_failed_sqs_message_count = response.batch_item_failures.len(),
        translation_guard_rejection_count = guard_rejection_count,
        translation_retry_count = retry_count,
        translation_batch_duration_ms = started_at.elapsed().as_millis(),
        "Finished ProductListing translation batch"
    );
    Ok(response)
}

fn is_guard_rejection(outcome: &RecordOutcome<ProductTranslationJobDisposition>) -> bool {
    matches!(
        outcome,
        RecordOutcome::Completed(ProductTranslationJobDisposition::Complete(
            "duplicate" | "stale" | "ignored_event"
        ))
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductTranslationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(FailureDiagnostic),
    Poison(&'static str),
}

impl ProductTranslationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
            Self::DependencyUnavailable(_) => "translation_unavailable",
        }
    }

    pub const fn failure_diagnostic(&self) -> Option<&FailureDiagnostic> {
        match self {
            Self::DependencyUnavailable(failure) => Some(failure),
            Self::Complete(_) | Self::Retry(_) | Self::Poison(_) => None,
        }
    }
}

pub async fn process_product_translation_job(
    body: &str,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
) -> ProductTranslationJobDisposition {
    match decode(body, WorkerScope::ProductListingTranslation) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductTranslationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
    job: DomainJob,
) -> ProductTranslationJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductTranslationJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-translation:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => translation_disposition(result.outcome),
        Err(error) => translation_error_disposition(&error),
    }
}

/// Source, provider, write, and commit failures retain the job. A completed provider call is
/// deliberately indistinguishable from unknown unfinished work until the guarded commit. Only the
/// failed stage and safe cause facts are kept; the error chain itself is never logged.
fn translation_error_disposition(
    error: &TranslateProductListingEventError,
) -> ProductTranslationJobDisposition {
    use ProductTranslationJobDisposition as D;
    use TranslateProductListingEventError as E;
    let stage = match error {
        E::ServiceOrSystemPrincipalRequired => return D::Poison("system_principal_required"),
        E::TranslationFailed { .. } => return D::DependencyUnavailable(translation_failure(error)),
        E::SourceReadFailed { .. } => "source_read",
        E::BeginTransactionFailed { .. } => "write_begin",
        E::TranslationWriteFailed { .. } => "translation_write",
        E::CommitTransactionFailed { .. } => "write_commit",
    };
    D::DependencyUnavailable(FailureDiagnostic::postgres(stage, error))
}

/// Classifies by the provider error when one is in the chain. Otherwise the translator adapter
/// rejected the provider response itself, which its port variant still names.
fn translation_failure(error: &TranslateProductListingEventError) -> FailureDiagnostic {
    let chain = || {
        std::iter::successors(Some(error as &(dyn std::error::Error + 'static)), |error| {
            (*error).source()
        })
    };
    let category = chain()
        .find_map(|error| error.downcast_ref::<LargeLanguageModelError>())
        .map(LargeLanguageModelError::diagnostic_category)
        .or_else(|| {
            chain()
                .find_map(|error| error.downcast_ref::<ProductListingTitleTranslationError>())
                .map(|error| match error {
                    ProductListingTitleTranslationError::TemporarilyUnavailable { .. } => {
                        "unavailable"
                    }
                    ProductListingTitleTranslationError::InvalidResponse { .. } => {
                        "response_invalid"
                    }
                })
        })
        .unwrap_or(UNCLASSIFIED_FAILURE);
    FailureDiagnostic::new("translation", "anthropic", category, None)
}

pub fn translation_disposition(
    outcome: TranslateProductListingEventOutcome,
) -> ProductTranslationJobDisposition {
    use TranslateProductListingEventOutcome as O;
    match outcome {
        O::Applied => ProductTranslationJobDisposition::Complete("applied"),
        O::Duplicate => ProductTranslationJobDisposition::Complete("duplicate"),
        O::Stale => ProductTranslationJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductTranslationJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductTranslationJobDisposition::Complete("missing_title"),
        O::MissingTitleLanguage => {
            ProductTranslationJobDisposition::Complete("missing_title_language")
        }
        O::EmptyTitle => ProductTranslationJobDisposition::Complete("empty_title"),
        O::ProductListingNotFound => ProductTranslationJobDisposition::Retry("missing_source"),
    }
}

pub fn command_from_job(
    job: DomainJob,
) -> Result<TranslateProductListingCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(TranslateProductListingCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::OperationContext;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use product_listing_service::use_cases::{
        TranslateProductListingCommand, TranslateProductListingEventError,
        TranslateProductListingEventOutcome, TranslateProductListingEventResult,
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
    async fn acknowledges_committed_and_authoritative_noop_outcomes_but_retries_unfinished_work() {
        let use_case = FakeUseCase::new([
            FakeResult::Outcome(TranslateProductListingEventOutcome::Applied),
            FakeResult::Outcome(TranslateProductListingEventOutcome::Duplicate),
            FakeResult::Outcome(TranslateProductListingEventOutcome::Stale),
            FakeResult::Outcome(TranslateProductListingEventOutcome::IgnoredEvent),
            FakeResult::Outcome(TranslateProductListingEventOutcome::MissingTitle),
            FakeResult::Outcome(TranslateProductListingEventOutcome::MissingTitleLanguage),
            FakeResult::Outcome(TranslateProductListingEventOutcome::EmptyTitle),
            FakeResult::Outcome(TranslateProductListingEventOutcome::ProductListingNotFound),
            FakeResult::Unavailable,
        ]);

        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("ignored"), valid_body()),
                (Some("missing-title"), valid_body()),
                (Some("missing-language"), valid_body()),
                (Some("empty-title"), valid_body()),
                (Some("missing-source"), valid_body()),
                (Some("commit-unknown"), valid_body()),
                (Some("poison"), "not-json".to_owned()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(
            failure_ids(response),
            ["missing-source", "commit-unknown", "poison"]
        );
        assert_eq!(use_case.calls(), 9);
    }

    #[tokio::test]
    async fn retains_records_when_the_shared_or_per_record_budget_is_exhausted() {
        let unstarted = FakeUseCase::new([FakeResult::Outcome(
            TranslateProductListingEventOutcome::Applied,
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
    fn retains_every_record_when_postgres_or_credential_setup_cannot_complete() {
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
        let use_case = FakeUseCase::new([FakeResult::ProviderAuthenticationFailed]);
        let (logs, guard) = CapturedLogs::start();

        let response = handler(events([(Some("unavailable"), valid_body())]), &use_case)
            .await
            .expect("handler response");
        drop(guard);

        assert_eq!(failure_ids(response), ["unavailable"]);
        let retained = logs.events_where(
            "message",
            "Product translation record retained for SQS retry or redrive",
        );
        assert_eq!(1, retained.len());
        assert_eq!(Some("translation"), retained[0]["stage"].as_str());
        assert_eq!(Some("anthropic"), retained[0]["dependency"].as_str());
        assert_eq!(
            Some("authentication"),
            retained[0]["error_category"].as_str()
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"product-translation","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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
        context.deadline = epoch_millis().saturating_add(50_000);
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
        Outcome(TranslateProductListingEventOutcome),
        Unavailable,
        ProviderAuthenticationFailed,
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
    impl TranslateProductListingEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            _: TranslateProductListingCommand,
        ) -> Result<TranslateProductListingEventResult, TranslateProductListingEventError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = self.results.lock().expect("results lock").pop_front();
            match result {
                Some(FakeResult::Outcome(outcome)) => {
                    Ok(TranslateProductListingEventResult { outcome })
                }
                Some(FakeResult::Unavailable) => {
                    Err(TranslateProductListingEventError::CommitTransactionFailed {
                        source: Box::new(std::io::Error::other("commit result unknown")),
                    })
                }
                Some(FakeResult::ProviderAuthenticationFailed) => {
                    Err(TranslateProductListingEventError::TranslationFailed {
                        source: Box::new(LargeLanguageModelError::Authentication {
                            source: Box::new(std::io::Error::other(SENTINEL)),
                        }),
                    })
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
