use aura_historia_worker::product_listing_opensearch::{
    ProductListingOpenSearchJobDisposition, process_product_listing_opensearch_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use lambda_runtime::{Error, LambdaEvent};
use opensearch::OpenSearch;
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_postgres::SqlxUnitOfWork;
use product_listing_opensearch::OpenSearchProductListingSearchProjection;
use product_listing_postgres::SqlxProductListingSearchFilterMatchSourceReaderFactory;
use product_listing_service::use_cases::{
    ProjectProductListingHandler, ProjectProductListingUseCase,
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

/// Adapt native Lambda SQS records without exposing Lambda DTOs to the worker or service layers.
///
/// A missing message ID fails the whole invocation. Lambda then returns no partial-success
/// response, so records already handled in the same batch are retried rather than acknowledged.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

/// Production composition for the Lambda's one PostgreSQL-to-OpenSearch projection use case.
/// Integration tests call this same root; no test-only handler may diverge from production wiring.
pub fn compose_projection_use_case(
    pool: sqlx::PgPool,
    open_search: OpenSearch,
) -> Arc<dyn ProjectProductListingUseCase> {
    Arc::new(ProjectProductListingHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchProductListingSearchProjection::new(open_search),
    ))
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

/// Uses the budget created at the invocation edge so composition and record work share one deadline.
pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
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

/// Returns every valid record as unfinished when setup has consumed the invocation budget.
/// A missing message ID still fails the whole invocation because Lambda cannot form a truthful
/// partial batch response for it.
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
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
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
        let Some(processing_budget) =
            record_processing_budget(max_record_processing_budget, remaining)
        else {
            warn!(
                message_id = %message_id,
                outcome = "insufficient_invocation_budget",
                "ProductListing projection record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
            continue;
        };
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => ProductListingOpenSearchJobDisposition::Poison("missing_message_body"),
        };

        if !matches!(
            disposition,
            ProductListingOpenSearchJobDisposition::Complete(_)
        ) {
            warn!(
                message_id = %message_id,
                outcome = disposition.category(),
                "ProductListing projection record retained for SQS retry or redrive"
            );
            failures.push(batch_failure(message_id));
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished ProductListing OpenSearch projection batch"
    );

    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

fn record_processing_budget(maximum: Duration, remaining: Duration) -> Option<Duration> {
    let budget = maximum.min(remaining);
    (!budget.is_zero()).then_some(budget)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
    processing_budget: Duration,
) -> ProductListingOpenSearchJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_product_listing_opensearch_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => ProductListingOpenSearchJobDisposition::Retry("execution_timeout"),
        Err(_) => ProductListingOpenSearchJobDisposition::Retry("handler_panicked"),
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
    use product_listing_service::use_cases::{
        ProjectProductListingCommand, ProjectProductListingError, ProjectProductListingOutcome,
        ProjectProductListingResult,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    const EVENT_ID: &str = "evt_01h455vb4pex5vy7enb1p677vn";
    const PRODUCT_LISTING_ID: &str = "pl_01h455vb4pex5vy7enb1p677vn";

    #[tokio::test]
    async fn should_acknowledge_completed_duplicate_and_stale_projection_results() {
        let processor =
            FakeUseCase::new([FakeResult::Applied, FakeResult::Deleted, FakeResult::Stale]);

        let result = handler(
            events([
                (Some("completed"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("stale"), valid_body()),
            ]),
            &processor,
        )
        .await;

        let response = match result {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };
        assert!(response.batch_item_failures.is_empty());
        assert_eq!(processor.calls(), 3);
    }

    #[tokio::test]
    async fn should_preserve_worker_retry_poison_and_dependency_dispositions_from_the_lambda_entrypoint()
     {
        let processor = FakeUseCase::new([
            FakeResult::Applied,
            FakeResult::Stale,
            FakeResult::MissingSource,
            FakeResult::SaleObservationFxSnapshotMissing,
            FakeResult::SaleObservationFxSnapshotInvalid,
            FakeResult::Unavailable,
        ]);

        let response = match handler(
            events([
                (Some("applied"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("missing-source"), valid_body()),
                (Some("missing-fx"), valid_body()),
                (Some("invalid-fx"), valid_body()),
                (Some("unavailable"), valid_body()),
            ]),
            &processor,
        )
        .await
        {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };

        assert_eq!(
            message_ids(response),
            ["missing-source", "missing-fx", "invalid-fx", "unavailable"]
        );
        assert_eq!(processor.calls(), 6);
    }

    #[tokio::test]
    async fn should_fail_deferred_poison_and_malformed_records_by_message_id() {
        let processor = FakeUseCase::new([FakeResult::Applied, FakeResult::MissingSource]);

        let result = handler(
            events([
                (Some("completed"), valid_body()),
                (Some("deferred"), valid_body()),
                (Some("poison"), String::from("not-json")),
            ]),
            &processor,
        )
        .await;

        let response = match result {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };
        assert_eq!(message_ids(response), ["deferred", "poison"]);
        assert_eq!(processor.calls(), 2);
    }

    #[tokio::test]
    async fn should_retain_all_records_without_executing_when_composition_spent_the_budget() {
        let processor = FakeUseCase::new([FakeResult::Applied]);
        let mut event = events([
            (Some("unfinished-first"), valid_body()),
            (Some("unfinished-second"), valid_body()),
        ]);
        event.context.deadline = epoch_millis();
        let budget = invocation_budget(&event.context);

        let response = match handler_with_invocation_budget(event, &processor, &budget).await {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };

        assert_eq!(
            message_ids(response),
            ["unfinished-first", "unfinished-second"]
        );
        assert_eq!(processor.calls(), 0);
    }

    #[test]
    fn should_retain_each_valid_record_when_setup_cannot_complete() {
        let event = events([
            (Some("unfinished-first"), valid_body()),
            (Some("unfinished-second"), valid_body()),
        ]);

        let response = match retain_all_records(&event) {
            Ok(response) => response,
            Err(error) => panic!("failed to preserve batch: {error}"),
        };

        assert_eq!(
            message_ids(response),
            ["unfinished-first", "unfinished-second"]
        );
    }

    #[tokio::test]
    async fn should_fail_timeout_and_panic_without_acknowledging_the_record() {
        for result in [FakeResult::Pending, FakeResult::Panic] {
            let processor = FakeUseCase::new([result]);
            let response = match handler_with_budget(
                events([(Some("retry"), valid_body())]),
                &processor,
                Duration::from_millis(1),
                Duration::from_millis(1),
            )
            .await
            {
                Ok(response) => response,
                Err(error) => panic!("handler failed: {error}"),
            };
            assert_eq!(message_ids(response), ["retry"]);
        }
    }

    #[tokio::test]
    async fn should_use_the_record_cap_without_spending_the_shared_invocation_budget() {
        let processor = FakeUseCase::new([FakeResult::Pending, FakeResult::Applied]);

        let response = match handler_with_budget(
            events([
                (Some("bounded"), valid_body()),
                (Some("completed"), valid_body()),
            ]),
            &processor,
            Duration::from_millis(100),
            Duration::from_millis(1),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };

        assert_eq!(message_ids(response), ["bounded"]);
        assert_eq!(processor.calls(), 2);
    }

    #[tokio::test]
    async fn should_fail_each_later_record_without_starting_it_when_shared_budget_is_spent() {
        let processor = FakeUseCase::new([FakeResult::Pending, FakeResult::Applied]);

        let response = match handler_with_budget(
            events([
                (Some("timed-out"), valid_body()),
                (Some("unstarted"), valid_body()),
            ]),
            &processor,
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };

        assert_eq!(message_ids(response), ["timed-out", "unstarted"]);
        assert_eq!(processor.calls(), 1);
    }

    #[tokio::test]
    async fn should_acknowledge_an_earlier_completed_record_and_retain_a_later_unfinished_record() {
        let processor = FakeUseCase::new([FakeResult::Applied, FakeResult::Pending]);

        let response = match handler_with_budget(
            events([
                (Some("completed"), valid_body()),
                (Some("unfinished"), valid_body()),
            ]),
            &processor,
            Duration::from_secs(1),
            Duration::from_millis(1),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => panic!("handler failed: {error}"),
        };

        assert_eq!(message_ids(response), ["unfinished"]);
        assert_eq!(processor.calls(), 2);
    }

    #[tokio::test]
    async fn should_fail_whole_batch_when_a_record_has_no_message_id() {
        let processor = FakeUseCase::new([FakeResult::Applied]);

        let result = handler(
            events([
                (Some("completed-before-invalid"), valid_body()),
                (None, valid_body()),
            ]),
            &processor,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(processor.calls(), 1);
    }

    fn valid_body() -> String {
        format!(
            r#"{{"schema_version":2,"scope":"product-listing-opensearch","job_type":"PRODUCT_LISTING_EVENT","idempotency_key":"product-event:{EVENT_ID}","ordering_key":"product:{PRODUCT_LISTING_ID}","payload":{{"event_id":"{EVENT_ID}","product_listing_id":"{PRODUCT_LISTING_ID}"}}}}"#,
        )
    }

    fn events<const N: usize>(records: [(Option<&str>, String); N]) -> LambdaEvent<SqsEvent> {
        let messages = records
            .into_iter()
            .map(|(message_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = message_id.map(ToOwned::to_owned);
                message.body = Some(body);
                message
            })
            .collect();
        let mut event = SqsEvent::default();
        event.records = messages;
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(60_000);
        LambdaEvent::new(event, context)
    }

    fn epoch_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    fn message_ids(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Applied,
        Deleted,
        Stale,
        MissingSource,
        SaleObservationFxSnapshotMissing,
        SaleObservationFxSnapshotInvalid,
        Unavailable,
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
    impl ProjectProductListingUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: ProjectProductListingCommand,
        ) -> Result<ProjectProductListingResult, ProjectProductListingError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let next = match self.results.lock() {
                Ok(mut results) => results.pop_front(),
                Err(poisoned) => poisoned.into_inner().pop_front(),
            };
            match next {
                Some(FakeResult::Applied) => Ok(result(ProjectProductListingOutcome::Applied)),
                Some(FakeResult::Deleted) => Ok(result(ProjectProductListingOutcome::Deleted)),
                Some(FakeResult::Stale) => Ok(result(ProjectProductListingOutcome::Stale)),
                Some(FakeResult::MissingSource) => {
                    Ok(result(ProjectProductListingOutcome::MissingSource))
                }
                Some(FakeResult::SaleObservationFxSnapshotMissing) => {
                    Err(ProjectProductListingError::SaleObservationFxSnapshotMissing)
                }
                Some(FakeResult::SaleObservationFxSnapshotInvalid) => Err(
                    ProjectProductListingError::SaleObservationFxSnapshotInvalid {
                        source: Box::new(std::io::Error::other("invalid FX snapshot")),
                    },
                ),
                Some(FakeResult::Unavailable) => {
                    Err(ProjectProductListingError::SourceReadFailed {
                        source: Box::new(std::io::Error::other("projection source unavailable")),
                    })
                }
                Some(FakeResult::Pending) => std::future::pending().await,
                Some(FakeResult::Panic) => panic!("projection handler panic"),
                None => panic!("missing fake projection result"),
            }
        }
    }

    fn result(outcome: ProjectProductListingOutcome) -> ProjectProductListingResult {
        ProjectProductListingResult { outcome }
    }

    #[test]
    fn should_keep_wire_fixture_ids_canonical() {
        assert!(EVENT_ID.parse::<EventId>().is_ok());
        assert!(PRODUCT_LISTING_ID.parse::<ProductListingId>().is_ok());
    }
}
