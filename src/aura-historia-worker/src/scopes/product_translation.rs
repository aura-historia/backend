//! `product-translation`: translates one product listing's title through Anthropic behind a
//! first-committed-completion guard.

use crate::composition::CompositionError;
use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use large_language_model::{
    AnthropicClaude, AnthropicConfig, ClaudeHaiku55Thinking, ClaudeModel,
    ClaudeReducedThinkingEffort, LargeLanguageModel, LargeLanguageModelError,
};
use platform_postgres::SqlxUnitOfWork;
use platform_postgres_queue::Disposition;
use platform_process::{Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE, config::required};
use platform_worker::{Job, JobHandler, error_class};
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
use sqlx::PgPool;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};

const SCOPE: &str = "product-translation";
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
// Title translation needs no reasoning, so thinking stays off and the translator's output-token
// limit remains reserved for the structured answer.
const TRANSLATION_MODEL: ClaudeModel = ClaudeModel::Haiku55(ClaudeHaiku55Thinking::Disabled(
    ClaudeReducedThinkingEffort::Low,
));

/// Builds the PostgreSQL-backed, first-committed-completion translation use case.
///
/// The source read completes before inference and the guarded write transaction starts only after
/// it, so no pool connection is retained while Claude is awaited.
pub fn compose_product_translation_use_case(
    pool: PgPool,
    translator: AnthropicClaude,
) -> Arc<dyn TranslateProductListingEventUseCase> {
    compose_product_translation_use_case_with_model(pool, translator)
}

pub fn compose_product_translation_use_case_with_model<L>(
    pool: PgPool,
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

    /// The guarded write rejected the job as already settled.
    fn is_guard_rejection(self) -> bool {
        matches!(
            self,
            Self::Complete("duplicate" | "stale" | "ignored_event")
        )
    }
}

pub async fn process_product_translation_job(
    body: &str,
    use_case: &dyn TranslateProductListingEventUseCase,
) -> ProductTranslationJobDisposition {
    match decode(body, WorkerScope::ProductListingTranslation) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductTranslationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job(
    use_case: &dyn TranslateProductListingEventUseCase,
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
    let category = find_source::<LargeLanguageModelError>(error)
        .map(LargeLanguageModelError::diagnostic_category)
        .or_else(|| {
            find_source::<ProductListingTitleTranslationError>(error).map(|error| match error {
                ProductListingTitleTranslationError::TemporarilyUnavailable { .. } => "unavailable",
                ProductListingTitleTranslationError::InvalidResponse { .. } => "response_invalid",
            })
        })
        .unwrap_or(UNCLASSIFIED_FAILURE);
    FailureDiagnostic::new("translation", Dependency::Anthropic, category, None)
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

/// Only completion acknowledges. Every other outcome, including poison, is retried under its
/// category until the queue moves it to custody; the error chain itself is never logged.
pub fn disposition(
    job: &Job,
    outcome: ProductTranslationJobDisposition,
    duration: Duration,
) -> Disposition {
    let category = outcome.category();
    if matches!(outcome, ProductTranslationJobDisposition::Complete(_)) {
        info!(
            job_id = %job.id(),
            outcome = category,
            guard_rejection = outcome.is_guard_rejection(),
            processing_duration_ms = duration.as_millis(),
            "Product translation job completed"
        );
        return Disposition::Complete;
    }
    let failure = outcome.failure_diagnostic();
    warn!(
        job_id = %job.id(),
        attempt = job.attempt(),
        outcome = category,
        stage = failure.map(FailureDiagnostic::stage),
        dependency = failure.and_then(FailureDiagnostic::dependency),
        error_category = failure.map(FailureDiagnostic::category),
        http_status = failure.and_then(FailureDiagnostic::http_status),
        sqlstate = failure.and_then(FailureDiagnostic::sqlstate),
        processing_duration_ms = duration.as_millis(),
        "Product translation job retained for retry"
    );
    Disposition::Retry(error_class(category))
}

pub struct ProductTranslationJobs {
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
}

impl ProductTranslationJobs {
    pub fn new(use_case: Arc<dyn TranslateProductListingEventUseCase>) -> Self {
        Self { use_case }
    }
}

#[async_trait]
impl JobHandler for ProductTranslationJobs {
    async fn handle(&self, job: &Job) -> Disposition {
        let started_at = Instant::now();
        let outcome = process_product_translation_job(job.payload(), self.use_case.as_ref()).await;
        disposition(job, outcome, started_at.elapsed())
    }
}

pub async fn compose(pool: &PgPool) -> Result<Arc<dyn JobHandler>, CompositionError> {
    let api_key = required(ANTHROPIC_API_KEY_ENV).map_err(|e| CompositionError::new(SCOPE, e))?;
    // The client error is replaced, never chained, so no part of the key can reach a log.
    let model = AnthropicClaude::new(AnthropicConfig::new(TRANSLATION_MODEL), &api_key)
        .map_err(|_| CompositionError::new(SCOPE, "failed to configure the Anthropic client"))?;
    Ok(Arc::new(ProductTranslationJobs::new(
        compose_product_translation_use_case(pool.clone(), model),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scopes::test_support::claimed_job;
    use product_listing_service::use_cases::TranslateProductListingEventResult;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use test_api::CapturedLogs;

    const RETAINED: &str = "Product translation job retained for retry";
    const COMPLETED: &str = "Product translation job completed";
    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    #[tokio::test]
    async fn acknowledges_committed_and_authoritative_noop_outcomes_but_retries_unfinished_work() {
        use TranslateProductListingEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Applied),
            FakeResult::Outcome(O::Duplicate),
            FakeResult::Outcome(O::Stale),
            FakeResult::Outcome(O::IgnoredEvent),
            FakeResult::Outcome(O::MissingTitle),
            FakeResult::Outcome(O::MissingTitleLanguage),
            FakeResult::Outcome(O::EmptyTitle),
            FakeResult::Outcome(O::ProductListingNotFound),
            FakeResult::Unavailable,
            FakeResult::PrincipalRejected,
        ]));
        let handler = ProductTranslationJobs::new(use_case.clone());

        let mut dispositions = Vec::new();
        for _ in 0..10 {
            dispositions.push(handler.handle(&claimed_job(VALID_BODY)).await);
        }
        dispositions.push(handler.handle(&claimed_job("not-json")).await);

        assert_eq!(
            vec![
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Complete,
                Disposition::Retry(error_class("missing_source")),
                Disposition::Retry(error_class("translation_unavailable")),
                Disposition::Retry(error_class("system_principal_required")),
                Disposition::Retry(error_class("invalid_wire_job")),
            ],
            dispositions
        );
        assert_eq!(use_case.calls(), 10);
    }

    #[test]
    fn should_retry_unexpected_payloads_under_their_category() {
        assert_eq!(
            Disposition::Retry(error_class("unexpected_payload")),
            disposition(
                &claimed_job(VALID_BODY),
                ProductTranslationJobDisposition::Poison("unexpected_payload"),
                Duration::ZERO,
            )
        );
    }

    #[tokio::test]
    async fn should_mark_guard_rejections_on_completed_jobs() {
        use TranslateProductListingEventOutcome as O;
        let use_case = Arc::new(FakeUseCase::new([
            FakeResult::Outcome(O::Applied),
            FakeResult::Outcome(O::Stale),
        ]));
        let handler = ProductTranslationJobs::new(use_case);
        let (logs, guard) = CapturedLogs::start();

        handler.handle(&claimed_job(VALID_BODY)).await;
        handler.handle(&claimed_job(VALID_BODY)).await;
        drop(guard);

        let completed = logs.events_where("message", COMPLETED);
        assert_eq!(2, completed.len());
        assert_eq!(Some("applied"), completed[0]["outcome"].as_str());
        assert_eq!(Some(false), completed[0]["guard_rejection"].as_bool());
        assert_eq!(Some("stale"), completed[1]["outcome"].as_str());
        assert_eq!(Some(true), completed[1]["guard_rejection"].as_bool());
    }

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let use_case = Arc::new(FakeUseCase::new([FakeResult::ProviderAuthenticationFailed]));
        let job = claimed_job(VALID_BODY);
        let (logs, guard) = CapturedLogs::start();

        let disposition = ProductTranslationJobs::new(use_case).handle(&job).await;
        drop(guard);

        assert_eq!(
            Disposition::Retry(error_class("translation_unavailable")),
            disposition
        );
        let retained = logs.events_where("message", RETAINED);
        assert_eq!(1, retained.len());
        assert_eq!(
            Some(job.id().to_string().as_str()),
            retained[0]["job_id"].as_str()
        );
        assert_eq!(Some("translation"), retained[0]["stage"].as_str());
        assert_eq!(Some("anthropic"), retained[0]["dependency"].as_str());
        assert_eq!(
            Some("authentication"),
            retained[0]["error_category"].as_str()
        );
        assert!(!logs.text().contains("SENTINEL"));
    }

    const VALID_BODY: &str = r#"{"schema_version":1,"scope":"product-translation","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#;

    enum FakeResult {
        Outcome(TranslateProductListingEventOutcome),
        Unavailable,
        PrincipalRejected,
        ProviderAuthenticationFailed,
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
                Some(FakeResult::PrincipalRejected) => {
                    Err(TranslateProductListingEventError::ServiceOrSystemPrincipalRequired)
                }
                Some(FakeResult::ProviderAuthenticationFailed) => {
                    Err(TranslateProductListingEventError::TranslationFailed {
                        source: Box::new(LargeLanguageModelError::Authentication {
                            source: Box::new(std::io::Error::other(SENTINEL)),
                        }),
                    })
                }
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
