use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use product_core::product_id::ProductId;
use product_service::use_cases::{
    ProjectProductCommand, ProjectProductOutcome, ProjectProductUseCase,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

pub async fn consume_product_opensearch_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn ProjectProductUseCase>,
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
                info!(job_type = "product_opensearch", %idempotency_key, %ordering_key, ?outcome, "Product OpenSearch projection job completed")
            }
            (Ok(()), None) => {
                error!(job_type = "product_opensearch", %idempotency_key, %ordering_key, outcome = "missing", "Product OpenSearch projection job completed without an outcome")
            }
            (Err(error), _) => {
                error!(job_type = "product_opensearch", %idempotency_key, %ordering_key, error = %error, outcome = "dead_lettered_in_memory", "Product OpenSearch projection job failed")
            }
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn ProjectProductUseCase>,
    job: DomainJob,
    outcome: Arc<Mutex<Option<ProjectProductOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let result = use_case.execute(command).await.map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
}

fn command_from_job(job: DomainJob) -> Result<ProjectProductCommand, ProductOpenSearchWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(ProductOpenSearchWorkerError::UnexpectedJobPayload);
    };
    Ok(ProjectProductCommand {
        event_id: EventId::try_from(event.event_id.as_str()).map_err(|source| {
            ProductOpenSearchWorkerError::InvalidEventId {
                source: box_error(source),
            }
        })?,
        product_id: ProductId::try_from(event.product_id.as_str()).map_err(|source| {
            ProductOpenSearchWorkerError::InvalidProductId {
                source: box_error(source),
            }
        })?,
    })
}

#[derive(Debug, thiserror::Error)]
enum ProductOpenSearchWorkerError {
    #[error("Product OpenSearch queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("Product OpenSearch job has an invalid event ID")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("Product OpenSearch job has an invalid Product ID")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
}
