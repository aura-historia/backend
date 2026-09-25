use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use platform_lambda_sqs::{RecordOutcome, process_batch};
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
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationResult,
    GenerateSearchFilterMatchNotificationUseCase,
};

use std::{sync::Arc, time::Duration};
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
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
    invocation_budget: Duration,
    max_record_processing_budget: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_batch(
        event,
        invocation_budget,
        max_record_processing_budget,
        |body| async move { process_search_filter_match_notification_job(&body, use_case).await },
    )
    .await?;
    let record_count = results.record_count;
    let response = results.finish(|attempt| {
        let (complete, category) = match &attempt.outcome {
            RecordOutcome::Completed(disposition) => (
                matches!(
                    disposition,
                    SearchFilterMatchNotificationJobDisposition::Complete(_)
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
                "Saved-filter match-notification record retained for SQS retry or redrive");
        }
        complete
    });
    info!(
        sqs_message_count = record_count,
        failed_sqs_message_count = response.batch_item_failures.len(),
        "Finished saved-filter match-notification batch"
    );
    Ok(response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterMatchNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterMatchNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn process_search_filter_match_notification_job(
    body: &str,
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
) -> SearchFilterMatchNotificationJobDisposition {
    match decode::<aura_historia_jobs::SearchFilterOperation>(
        body,
        WorkerScope::SearchFilterMatchNotification,
    ) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterMatchNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

pub async fn execute_job<O>(
    use_case: &(dyn GenerateSearchFilterMatchNotificationUseCase + Send + Sync),
    job: DomainJob<O>,
) -> SearchFilterMatchNotificationJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return SearchFilterMatchNotificationJobDisposition::Poison("match_metadata_invalid");
    };
    match use_case.execute(command).await {
        Ok(result) => notification_outcome(result),
        Err(error) => {
            use GenerateSearchFilterMatchNotificationError as E;
            match error {
                E::MatchSourceStateInvalid { .. }
                | E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::ContentAssessmentStateInvalid { .. } => {
                    SearchFilterMatchNotificationJobDisposition::Poison(
                        "match_notification_state_invalid",
                    )
                }
                _ => SearchFilterMatchNotificationJobDisposition::DependencyUnavailable(
                    "match_notification_unavailable",
                ),
            }
        }
    }
}
pub fn notification_outcome(
    result: GenerateSearchFilterMatchNotificationResult,
) -> SearchFilterMatchNotificationJobDisposition {
    use GenerateSearchFilterMatchNotificationResult as R;
    match result {
        R::Created => SearchFilterMatchNotificationJobDisposition::Complete("inserted"),
        R::AlreadyExists => SearchFilterMatchNotificationJobDisposition::Complete("duplicate"),
        R::SuppressedByQuota => {
            SearchFilterMatchNotificationJobDisposition::Complete("suppressed_by_quota")
        }
        // User deletion is a terminal recipient suppression, not missing historical business truth.
        R::SuppressedForMissingUser => {
            SearchFilterMatchNotificationJobDisposition::Complete("missing_user")
        }
        R::SuppressedForWithdrawnProductListing => {
            SearchFilterMatchNotificationJobDisposition::Complete("withdrawn")
        }
        R::SuppressedForStaleMatch => {
            SearchFilterMatchNotificationJobDisposition::Complete("stale_match")
        }
        R::SuppressedForMissingMatch => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_match")
        }
        R::SuppressedForMissingProductListing => {
            SearchFilterMatchNotificationJobDisposition::Retry("missing_product")
        }
    }
}
pub fn command_from_job<O>(
    job: DomainJob<O>,
) -> Result<GenerateSearchFilterMatchNotificationCommand, aura_historia_jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(GenerateSearchFilterMatchNotificationCommand {
        user_id: change.user_id,
        search_filter_id: change.user_search_filter_id,
        product_listing_id: change.product_listing_id,
        origin_event_id: change.origin_event_id,
    })
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
