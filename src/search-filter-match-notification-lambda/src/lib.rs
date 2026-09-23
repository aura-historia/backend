use aura_historia_worker::search_filter_match_notifications::{
    SearchFilterMatchNotificationJobDisposition, process_search_filter_match_notification_job,
};
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use lambda_runtime::{Error, LambdaEvent};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_postgres::SqlxUnitOfWork;

use product_listing_postgres::{
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};

use search_filter_postgres::{
    SqlxSearchFilterMatchNotificationSourceReaderFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationUseCase,
};

use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, warn};

use user_postgres::SqlxUserTierEntitlementsFactory;

const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(40);

/// Builds the PostgreSQL-only notification use case used by the native SQS Lambda.
pub fn compose_search_filter_match_notification_use_case(
    pool: sqlx::PgPool,
) -> Arc<dyn GenerateSearchFilterMatchNotificationUseCase> {
    Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterMatchNotificationSourceReaderFactory,
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
        SqlxProductListingContentAssessmentSnapshotReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    ))
}

/// Computes one deadline shared by setup, record work, and SQS response headroom.
pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, &budget).await
}

/// Uses the invocation-edge budget so setup and all records share the same deadline.
pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
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

/// Retains every record when PostgreSQL credentials or composition cannot complete safely.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    let failures = event
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
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
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
            warn!(message_id = %message_id, outcome = "insufficient_invocation_budget", "Saved-filter match-notification record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
            continue;
        }
        let disposition = match record.body {
            Some(body) => process_with_budget(&body, use_case, processing_budget).await,
            None => SearchFilterMatchNotificationJobDisposition::Poison("missing_message_body"),
        };
        if !matches!(
            disposition,
            SearchFilterMatchNotificationJobDisposition::Complete(_)
        ) {
            warn!(message_id = %message_id, outcome = disposition.category(), "Saved-filter match-notification record retained for SQS retry or redrive");
            failures.push(batch_failure(message_id));
        }
    }

    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = failures.len(),
        "Finished saved-filter match-notification batch"
    );
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failures;
    Ok(response)
}

async fn process_with_budget(
    body: &str,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
    processing_budget: Duration,
) -> SearchFilterMatchNotificationJobDisposition {
    match AssertUnwindSafe(tokio::time::timeout(
        processing_budget,
        process_search_filter_match_notification_job(body, use_case),
    ))
    .catch_unwind()
    .await
    {
        Ok(Ok(disposition)) => disposition,
        Ok(Err(_)) => SearchFilterMatchNotificationJobDisposition::Retry("execution_timeout"),
        Err(_) => SearchFilterMatchNotificationJobDisposition::Retry("handler_panicked"),
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
    use search_filter_service::use_cases::{
        GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
        GenerateSearchFilterMatchNotificationResult,
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
    async fn acknowledges_semantic_completion_and_retries_unfinished_or_poisoned_records() {
        let use_case = FakeUseCase::new([
            FakeResult::Result(GenerateSearchFilterMatchNotificationResult::Created),
            FakeResult::Result(GenerateSearchFilterMatchNotificationResult::AlreadyExists),
            FakeResult::Result(GenerateSearchFilterMatchNotificationResult::SuppressedByQuota),
            FakeResult::Result(
                GenerateSearchFilterMatchNotificationResult::SuppressedForMissingUser,
            ),
            FakeResult::Result(
                GenerateSearchFilterMatchNotificationResult::SuppressedForWithdrawnProductListing,
            ),
            FakeResult::Result(
                GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch,
            ),
            FakeResult::Result(
                GenerateSearchFilterMatchNotificationResult::SuppressedForMissingMatch,
            ),
            FakeResult::Result(
                GenerateSearchFilterMatchNotificationResult::SuppressedForMissingProductListing,
            ),
            FakeResult::StateInvalid,
            FakeResult::Unavailable,
        ]);
        let response = handler(
            events([
                (Some("created"), valid_body()),
                (Some("duplicate"), valid_body()),
                (Some("quota"), valid_body()),
                (Some("missing-user"), valid_body()),
                (Some("withdrawn"), valid_body()),
                (Some("stale"), valid_body()),
                (Some("missing-match"), valid_body()),
                (Some("missing-product"), valid_body()),
                (Some("state-invalid"), valid_body()),
                (Some("unavailable"), valid_body()),
                (Some("poison"), "not-json".to_owned()),
            ]),
            &use_case,
        )
        .await
        .expect("handler response");

        assert_eq!(
            failure_ids(response),
            [
                "missing-match",
                "missing-product",
                "state-invalid",
                "unavailable",
                "poison",
            ]
        );
        assert_eq!(use_case.calls(), 10);
    }

    #[tokio::test]
    async fn retains_records_when_the_shared_or_per_record_budget_is_exhausted() {
        let unstarted = FakeUseCase::new([FakeResult::Result(
            GenerateSearchFilterMatchNotificationResult::Created,
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

    fn valid_body() -> String {
        r#"{"schema_version":2,"scope":"search-filter-match-notification","job_type":"SEARCH_FILTER_MATCH_CREATED","idempotency_key":"search-filter-match:usr_01h455vb4pex5vy7enb1p677vn:sf_01h455vb4pex5vy7enb1p677vn:pl_01h455vb4pex5vy7enb1p677vn:evt_01h455vb4pex5vy7enb1p677vn","ordering_key":"user:usr_01h455vb4pex5vy7enb1p677vn","payload":{"user_id":"usr_01h455vb4pex5vy7enb1p677vn","user_search_filter_id":"sf_01h455vb4pex5vy7enb1p677vn","product_listing_id":"pl_01h455vb4pex5vy7enb1p677vn","origin_event_id":"evt_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
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
        Result(GenerateSearchFilterMatchNotificationResult),
        StateInvalid,
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
    impl GenerateSearchFilterMatchNotificationUseCase for FakeUseCase {
        async fn execute(
            &self,
            _command: GenerateSearchFilterMatchNotificationCommand,
        ) -> Result<
            GenerateSearchFilterMatchNotificationResult,
            GenerateSearchFilterMatchNotificationError,
        > {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let result = { self.results.lock().expect("results lock").pop_front() };
            match result {
                Some(FakeResult::Result(result)) => Ok(result),
                Some(FakeResult::StateInvalid) => {
                    Err(GenerateSearchFilterMatchNotificationError::ProductListingSourceMismatch)
                }
                Some(FakeResult::Unavailable) => Err(
                    GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed {
                        source: Box::new(std::io::Error::other("unavailable")),
                    },
                ),
                Some(FakeResult::Pending) => std::future::pending().await,
                None => panic!("unexpected use case invocation"),
            }
        }
    }
}
