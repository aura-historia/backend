use aura_historia_worker::product_embedding::{
    ProductEmbeddingJobDisposition, process_product_embedding_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use embedding::{EmbeddingGenerator, VertexAiEmbeddingGenerator};
use futures_util::FutureExt;
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::{
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
};
use product_listing_service::use_cases::{
    EmbedProductListingEventHandler, EmbedProductListingEventUseCase,
};
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
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
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let record_count = event.payload.records.len();
    let invocation_started_at = Instant::now();
    let mut failures = Vec::new();
    let mut guard_rejection_count = 0_usize;
    let mut retry_count = 0_usize;

    for record in event.payload.records {
        let message_id = record.message_id.ok_or_else(|| {
            Error::from("SQS event record has no message ID; fail whole invocation")
        })?;
        let remaining = invocation_budget.saturating_sub(invocation_started_at.elapsed());
        let processing_budget = max_record_processing_budget.min(remaining);
        if processing_budget.is_zero() {
            retry_count += 1;
            warn!(
                message_id = %message_id,
                outcome = "insufficient_invocation_budget",
                "Product embedding record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
            continue;
        }

        let processing_started_at = Instant::now();
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => ProductEmbeddingJobDisposition::Poison("missing_message_body"),
        };
        let processing_duration_ms = processing_started_at.elapsed().as_millis();
        if matches!(
            disposition,
            ProductEmbeddingJobDisposition::Complete("duplicate" | "stale" | "ignored_event")
        ) {
            guard_rejection_count += 1;
        }
        if !matches!(disposition, ProductEmbeddingJobDisposition::Complete(_)) {
            retry_count += 1;
            warn!(
                message_id = %message_id,
                outcome = disposition.category(),
                processing_duration_ms,
                "Product embedding record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
        } else {
            info!(
                message_id = %message_id,
                outcome = disposition.category(),
                processing_duration_ms,
                "Product embedding record completed"
            );
        }
    }

    info!(
        embedding_sqs_message_count = record_count,
        embedding_failed_sqs_message_count = failures.len(),
        embedding_guard_rejection_count = guard_rejection_count,
        embedding_retry_count = retry_count,
        embedding_batch_duration_ms = invocation_started_at.elapsed().as_millis(),
        "Finished ProductListing embedding batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
    processing_budget: Duration,
) -> ProductEmbeddingJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_product_embedding_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => ProductEmbeddingJobDisposition::Retry("execution_timeout"),
        Err(_) => ProductEmbeddingJobDisposition::Retry("handler_panicked"),
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
