use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    ProjectProductListingCommand, ProjectProductListingOutcome, ProjectProductListingUseCase,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

pub async fn consume_product_listing_opensearch_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn ProjectProductListingUseCase>,
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
            (Ok(()), Some(outcome)) => {
                info!(job_type = "product_listing_opensearch", %idempotency_key, %ordering_key, ?outcome, "ProductListing OpenSearch projection job completed")
            }
            (Ok(()), None) => {
                error!(job_type = "product_listing_opensearch", %idempotency_key, %ordering_key, outcome = "missing", "ProductListing OpenSearch projection job completed without an outcome")
            }
            (Err(error), _) => {
                error!(job_type = "product_listing_opensearch", %idempotency_key, %ordering_key, error = %error, outcome = "dead_lettered_in_memory", "ProductListing OpenSearch projection job failed")
            }
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn ProjectProductListingUseCase>,
    job: DomainJob,
    outcome: Arc<Mutex<Option<ProjectProductListingOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let result = use_case.execute(command).await.map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
}

fn command_from_job(
    job: DomainJob,
) -> Result<ProjectProductListingCommand, ProductListingOpenSearchWorkerError> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(ProductListingOpenSearchWorkerError::UnexpectedJobPayload);
    };
    Ok(ProjectProductListingCommand {
        event_id: EventId::try_from(event.event_id.as_str()).map_err(|source| {
            ProductListingOpenSearchWorkerError::InvalidEventId {
                source: box_error(source),
            }
        })?,
        product_id: ProductListingId::try_from(event.product_id.as_str()).map_err(|source| {
            ProductListingOpenSearchWorkerError::InvalidProductListingId {
                source: box_error(source),
            }
        })?,
    })
}

#[derive(Debug, thiserror::Error)]
enum ProductListingOpenSearchWorkerError {
    #[error("ProductListing OpenSearch queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("ProductListing OpenSearch job has an invalid event ID")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing OpenSearch job has an invalid ProductListing ID")]
    InvalidProductListingId {
        #[source]
        source: BoxError,
    },
}
