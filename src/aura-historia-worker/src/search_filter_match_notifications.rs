use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use product_core::product_id::ProductId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationResult,
    GenerateSearchFilterMatchNotificationUseCase,
};
use std::sync::Arc;
use tracing::{Span, error, info};
use user_core::user_id::UserId;

pub async fn consume_search_filter_match_notification_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let identity = match_job_identity(&job);
        let use_case_for_retry = Arc::clone(&use_case);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let use_case = Arc::clone(&use_case_for_retry);
            async move { execute_job(use_case, job).await }
        })
        .await;

        match result {
            Ok(()) => info!(
                job_type = "search_filter_match_notification",
                %idempotency_key,
                %ordering_key,
                user_id = %identity.user_id,
                search_filter_id = %identity.search_filter_id,
                product_id = %identity.product_id,
                origin_event_id = %identity.origin_event_id,
                outcome = "applied",
                "search filter match notification job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_match_notification",
                %idempotency_key,
                %ordering_key,
                user_id = %identity.user_id,
                search_filter_id = %identity.search_filter_id,
                product_id = %identity.product_id,
                origin_event_id = %identity.origin_event_id,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter match notification job failed"
            ),
        }
    }
}

#[tracing::instrument(
    name = "process_search_filter_match_notification_job",
    skip(use_case, job),
    fields(
        user_id = tracing::field::Empty,
        search_filter_id = tracing::field::Empty,
        product_id = tracing::field::Empty,
        origin_event_id = tracing::field::Empty,
    )
)]
async fn execute_job(
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    job: DomainJob,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let span = Span::current();
    span.record("user_id", tracing::field::display(command.user_id));
    span.record(
        "search_filter_id",
        tracing::field::display(command.search_filter_id),
    );
    span.record("product_id", tracing::field::display(command.product_id));
    span.record(
        "origin_event_id",
        tracing::field::display(command.origin_event_id),
    );
    let result = use_case.execute(command).await.map_err(box_error)?;
    let notification_outcome = match result {
        GenerateSearchFilterMatchNotificationResult::Created => "inserted",
        GenerateSearchFilterMatchNotificationResult::AlreadyExists => "deduplicated",
        GenerateSearchFilterMatchNotificationResult::SuppressedByQuota => "suppressed_by_quota",
        GenerateSearchFilterMatchNotificationResult::SuppressedForMissingUser => {
            "suppressed_for_missing_user"
        }
        GenerateSearchFilterMatchNotificationResult::SuppressedForMissingMatch => {
            "suppressed_for_missing_match"
        }
        GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch => {
            "suppressed_for_stale_match"
        }
        GenerateSearchFilterMatchNotificationResult::SuppressedForMissingProduct => {
            "suppressed_for_missing_product"
        }
        GenerateSearchFilterMatchNotificationResult::SuppressedForStaleProductEvent => {
            "suppressed_for_stale_product_event"
        }
    };
    info!(
        job_type = "search_filter_match_notification",
        notification_outcome, "search filter match notification write completed"
    );
    Ok(())
}

#[derive(Debug, Default)]
struct SearchFilterMatchNotificationJobIdentity {
    user_id: String,
    search_filter_id: String,
    product_id: String,
    origin_event_id: String,
}

fn match_job_identity(job: &DomainJob) -> SearchFilterMatchNotificationJobIdentity {
    let DomainJobPayload::SearchFilterMatchCreated(change) = &job.payload else {
        return SearchFilterMatchNotificationJobIdentity::default();
    };

    SearchFilterMatchNotificationJobIdentity {
        user_id: change.user_id.clone(),
        search_filter_id: change.user_search_filter_id.clone(),
        product_id: change.product_id.clone(),
        origin_event_id: change.origin_event_id.clone(),
    }
}

fn command_from_job(
    job: DomainJob,
) -> Result<GenerateSearchFilterMatchNotificationCommand, SearchFilterMatchNotificationWorkerError>
{
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(SearchFilterMatchNotificationWorkerError::UnexpectedJobPayload);
    };
    let user_id = UserId::try_from(change.user_id.as_str()).map_err(|source| {
        SearchFilterMatchNotificationWorkerError::InvalidUserId {
            source: box_error(source),
        }
    })?;
    let search_filter_id = UserSearchFilterId::try_from(change.user_search_filter_id.as_str())
        .map_err(
            |source| SearchFilterMatchNotificationWorkerError::InvalidSearchFilterId {
                source: box_error(source),
            },
        )?;
    let product_id = ProductId::try_from(change.product_id.as_str()).map_err(|source| {
        SearchFilterMatchNotificationWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;
    let origin_event_id = EventId::try_from(change.origin_event_id.as_str()).map_err(|source| {
        SearchFilterMatchNotificationWorkerError::InvalidOriginEventId {
            source: box_error(source),
        }
    })?;

    Ok(GenerateSearchFilterMatchNotificationCommand {
        user_id,
        search_filter_id,
        product_id,
        origin_event_id,
    })
}

#[derive(Debug, thiserror::Error)]
enum SearchFilterMatchNotificationWorkerError {
    #[error("search filter match notification queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("search filter match notification job has an invalid user id")]
    InvalidUserId {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification job has an invalid search filter id")]
    InvalidSearchFilterId {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification job has an invalid origin event id")]
    InvalidOriginEventId {
        #[source]
        source: BoxError,
    },
}
