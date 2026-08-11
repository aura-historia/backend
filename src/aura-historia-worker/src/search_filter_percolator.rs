use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    product_id::ProductId,
};
use search_filter_service::use_cases::{MatchProductEventCommand, MatchProductEventUseCase};
use std::sync::Arc;
use tracing::{error, info};

pub async fn consume_search_filter_percolator_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn MatchProductEventUseCase>,
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
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                outcome = "applied",
                "search filter percolator job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter percolator job failed"
            ),
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn MatchProductEventUseCase>,
    job: DomainJob,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    use_case
        .execute(command)
        .await
        .map(|_| ())
        .map_err(box_error)
}

fn command_from_job(
    job: DomainJob,
) -> Result<MatchProductEventCommand, SearchFilterPercolatorWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(SearchFilterPercolatorWorkerError::UnexpectedJobPayload);
    };
    let origin_event_id = EventId::try_from(event.event_id.as_str()).map_err(|source| {
        SearchFilterPercolatorWorkerError::InvalidEventId {
            source: box_error(source),
        }
    })?;
    let product_id = ProductId::try_from(event.product_id.as_str()).map_err(|source| {
        SearchFilterPercolatorWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;

    Ok(MatchProductEventCommand {
        origin_event_id,
        product_id,
    })
}

#[derive(Debug, thiserror::Error)]
enum SearchFilterPercolatorWorkerError {
    #[error("search filter percolator queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("search filter percolator job has an invalid event id")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolator job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
}
