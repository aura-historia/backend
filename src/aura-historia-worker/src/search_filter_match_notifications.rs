use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    product_id::ProductId,
    user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationResult,
    GenerateSearchFilterMatchNotificationUseCase,
};
use std::sync::Arc;
use tracing::{error, info};

pub async fn consume_search_filter_match_notification_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
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
                outcome = "applied",
                "search filter match notification job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_match_notification",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter match notification job failed"
            ),
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    job: DomainJob,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
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
        GenerateSearchFilterMatchNotificationResult::SuppressedForNonSelectedFilter => {
            "suppressed_for_non_selected_filter"
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
