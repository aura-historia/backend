use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use large_language_model::{LargeLanguageModel, VertexAiGemini};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingTranslationSourceReader, SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventError,
    TranslateProductListingEventHandler, TranslateProductListingEventOutcome,
    TranslateProductListingEventUseCase,
};
use product_listing_translation_llm::LargeLanguageModelProductListingTitleTranslator;
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

// The translator has a bounded 30-second provider request. The remaining envelope covers source
// reads, guarded persistence, token refresh, and returning the partial SQS failure response.
const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the existing PostgreSQL-backed, first-committed-completion translation use case.
///
/// The source read completes before inference and the guarded write transaction starts only after
/// it, so a reusable pool connection is not retained while Vertex is awaited.
pub fn compose_product_translation_use_case(
    pool: sqlx::PgPool,
    translator: VertexAiGemini,
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
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (matches!(disposition, ProductTranslationJobDisposition::Complete(_)), disposition.category()),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if matches!(&attempt.outcome, RecordOutcome::Completed(ProductTranslationJobDisposition::Complete("duplicate" | "stale" | "ignored_event"))) {
            guard_rejection_count += 1;
        }
        if !complete { retry_count += 1; }
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
        processing_duration_ms = attempt.duration.as_millis(),
                "Product translation record retained for SQS retry or redrive");
        }
        if complete { info!(message_id = %attempt.message_id, outcome = category, processing_duration_ms = attempt.duration.as_millis(), "Product translation record completed"); }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductTranslationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl ProductTranslationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_product_translation_job(
    body: &str,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
) -> ProductTranslationJobDisposition {
    match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::ProductListingTranslation,
    ) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductTranslationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O>(
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
    job: DomainJob<O>,
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
        Err(TranslateProductListingEventError::ServiceOrSystemPrincipalRequired) => {
            ProductTranslationJobDisposition::Poison("system_principal_required")
        }
        // Source, provider, write, and commit failures retain the job. A completed provider call
        // is deliberately indistinguishable from unknown unfinished work until the guarded commit.
        Err(_) => {
            ProductTranslationJobDisposition::DependencyUnavailable("translation_unavailable")
        }
    }
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

pub fn command_from_job<O>(
    job: DomainJob<O>,
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

    fn valid_body() -> String {
        r#"{"schema_version":2,"scope":"product-translation","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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
                Some(FakeResult::Pending) => std::future::pending().await,
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
