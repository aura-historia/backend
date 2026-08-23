use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use product_core::product_id::ProductId;
use search_filter_service::use_cases::{
    MatchProductEventCommand, MatchProductEventOutcome, MatchProductEventUseCase,
};
use std::sync::Arc;
use tokio::sync::Mutex;
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
        let outcome = Arc::new(Mutex::new(None));
        let outcome_for_retry = Arc::clone(&outcome);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let use_case = Arc::clone(&use_case_for_retry);
            let outcome = Arc::clone(&outcome_for_retry);
            async move { execute_job(use_case, job, outcome).await }
        })
        .await;

        match (result, outcome.lock().await.take()) {
            (Ok(()), Some(outcome)) => info!(
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                ?outcome,
                "search filter percolator job completed"
            ),
            (Ok(()), None) => error!(
                job_type = "search_filter_percolator",
                %idempotency_key,
                %ordering_key,
                outcome = "missing",
                "search filter percolator job completed without an outcome"
            ),
            (Err(error), _) => error!(
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
    outcome: Arc<Mutex<Option<MatchProductEventOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let result = use_case.execute(command).await.map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
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
