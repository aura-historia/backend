use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use embedding::{EmbeddingGenerator, VertexAiEmbeddingGenerator};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    EmbedProductListingCommand, EmbedProductListingEventError, EmbedProductListingEventHandler,
    EmbedProductListingEventOutcome, EmbedProductListingEventUseCase,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

// These limits reserve a five-second response margin around a 60-second provider invocation
// envelope: image fetch, one Vertex request, and the short guarded write.
const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(60);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(55);

/// Builds the existing PostgreSQL-backed, source-version-guarded embedding use case.
///
/// The source read completes before the provider call and the write transaction begins only after
/// it, so the pool's one connection is not occupied while Vertex is awaited.
pub fn compose_product_embedding_use_case(
    pool: sqlx::PgPool,
    generator: VertexAiEmbeddingGenerator,
) -> Arc<dyn EmbedProductListingEventUseCase> {
    compose_product_embedding_use_case_with_generator(pool, generator)
}

pub fn compose_product_embedding_use_case_with_generator<G>(
    pool: sqlx::PgPool,
    generator: G,
) -> Arc<dyn EmbedProductListingEventUseCase>
where
    G: EmbeddingGenerator + 'static,
{
    Arc::new(EmbedProductListingEventHandler::new(
        SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
        generator,
        SqlxUnitOfWork::new(pool),
        SqlxProductListingEmbeddingWriterFactory::new(),
    ))
}

/// Computes the usable Lambda deadline, including credential refresh and SQS response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
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

/// Retains every source record if credentials or the PostgreSQL composition cannot be ready.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_product_embedding_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let started_at = results.started_at;
    let mut guard_rejection_count = 0_usize;
    let mut retry_count = 0_usize;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (matches!(disposition, ProductEmbeddingJobDisposition::Complete(_)), disposition.category()),
            RecordOutcome::MissingBody => (false, "missing_message_body"),
            RecordOutcome::InsufficientBudget => (false, "insufficient_invocation_budget"),
            RecordOutcome::TimedOut => (false, "execution_timeout"),
            RecordOutcome::Panicked => (false, "handler_panicked"),
        };
        if matches!(&attempt.outcome, RecordOutcome::Completed(ProductEmbeddingJobDisposition::Complete("duplicate" | "stale" | "ignored_event"))) {
            guard_rejection_count += 1;
        }
        if !complete { retry_count += 1; }
        if !complete {
            warn!(message_id = %attempt.message_id, outcome = category,
        processing_duration_ms = attempt.duration.as_millis(),
                "Product embedding record retained for SQS retry or redrive");
        }
        if complete { info!(message_id = %attempt.message_id, outcome = category, processing_duration_ms = attempt.duration.as_millis(), "Product embedding record completed"); }
        complete
    });
    info!(
        embedding_sqs_message_count = record_count,
        embedding_failed_sqs_message_count = response.batch_item_failures.len(),
        embedding_guard_rejection_count = guard_rejection_count,
        embedding_retry_count = retry_count,
        embedding_batch_duration_ms = started_at.elapsed().as_millis(),
        "Finished ProductListing embedding batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl ProductEmbeddingJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_product_embedding_job(
    body: &str,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
) -> ProductEmbeddingJobDisposition {
    match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::ProductListingEmbedding,
    ) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductEmbeddingJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O>(
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
    job: DomainJob<O>,
) -> ProductEmbeddingJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductEmbeddingJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-embedding:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => embedding_disposition(result.outcome),
        Err(EmbedProductListingEventError::ServiceOrSystemPrincipalRequired) => {
            ProductEmbeddingJobDisposition::Poison("system_principal_required")
        }
        Err(EmbedProductListingEventError::InvalidInput { .. }) => {
            ProductEmbeddingJobDisposition::Poison("embedding_input_invalid")
        }
        // Provider, source, write, and commit failures leave the job retryable. In particular,
        // a provider result does not prove that the guarded PostgreSQL commit completed.
        Err(_) => ProductEmbeddingJobDisposition::DependencyUnavailable("embedding_unavailable"),
    }
}

pub fn embedding_disposition(
    outcome: EmbedProductListingEventOutcome,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventOutcome as O;
    match outcome {
        O::Applied => ProductEmbeddingJobDisposition::Complete("applied"),
        O::Duplicate => ProductEmbeddingJobDisposition::Complete("duplicate"),
        O::Stale => ProductEmbeddingJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductEmbeddingJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductEmbeddingJobDisposition::Complete("missing_title"),
        O::ProductListingNotFound => ProductEmbeddingJobDisposition::Retry("missing_source"),
    }
}

pub fn command_from_job<O>(
    job: DomainJob<O>,
) -> Result<EmbedProductListingCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(EmbedProductListingCommand {
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
        EmbedProductListingCommand, EmbedProductListingEventError, EmbedProductListingEventOutcome,
        EmbedProductListingEventResult,
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
    async fn acknowledges_committed_outcomes_and_retries_all_unfinished_records() {
        let use_case = FakeUseCase::new([
            FakeResult::Outcome(EmbedProductListingEventOutcome::Applied),
            FakeResult::Outcome(EmbedProductListingEventOutcome::Duplicate),
            FakeResult::Outcome(EmbedProductListingEventOutcome::Stale),
            FakeResult::Outcome(EmbedProductListingEventOutcome::IgnoredEvent),
            FakeResult::Outcome(EmbedProductListingEventOutcome::MissingTitle),
            FakeResult::Outcome(EmbedProductListingEventOutcome::ProductListingNotFound),
            FakeResult::Unavailable,
        ]);

        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("ignored"), valid_body()),
                (Some("missing-title"), valid_body()),
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
            EmbedProductListingEventOutcome::Applied,
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
        r#"{"schema_version":2,"scope":"product-embedding","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"product:pl_01h455vb4pex5vy7enb1p677vn","payload":{"event_id":"evt_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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
        context.deadline = epoch_millis().saturating_add(65_000);
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
        Outcome(EmbedProductListingEventOutcome),
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
    impl EmbedProductListingEventUseCase for FakeUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            _: EmbedProductListingCommand,
        ) -> Result<EmbedProductListingEventResult, EmbedProductListingEventError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = self.results.lock().expect("results lock").pop_front();
            match result {
                Some(FakeResult::Outcome(outcome)) => {
                    Ok(EmbedProductListingEventResult { outcome })
                }
                Some(FakeResult::Unavailable) => {
                    Err(EmbedProductListingEventError::GenerationFailed {
                        source: Box::new(std::io::Error::other("unavailable")),
                    })
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
