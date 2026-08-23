use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::{
    error::{BoxError, box_error},
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
};
use domain_primitives::event_id::EventId;
use product_core::product_id::ProductId;
use product_service::use_cases::{
    EmbedProductCommand, EmbedProductEventOutcome, EmbedProductEventUseCase,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

pub async fn consume_product_embedding_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn EmbedProductEventUseCase>,
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
                info!(job_type = "product_embedding", %idempotency_key, %ordering_key, ?outcome, "product embedding job completed")
            }
            (Ok(()), None) => {
                error!(job_type = "product_embedding", %idempotency_key, %ordering_key, outcome = "missing", "product embedding job completed without an outcome")
            }
            (Err(error), _) => {
                error!(job_type = "product_embedding", %idempotency_key, %ordering_key, error = %error, outcome = "dead_lettered_in_memory", "product embedding job failed")
            }
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn EmbedProductEventUseCase>,
    job: DomainJob,
    outcome: Arc<Mutex<Option<EmbedProductEventOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-embedding:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    let result = use_case
        .execute(&context, command)
        .await
        .map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
}

fn command_from_job(job: DomainJob) -> Result<EmbedProductCommand, ProductEmbeddingWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(ProductEmbeddingWorkerError::UnexpectedJobPayload);
    };
    let event_id = EventId::try_from(event.event_id.as_str()).map_err(|source| {
        ProductEmbeddingWorkerError::InvalidEventId {
            source: box_error(source),
        }
    })?;
    let product_id = ProductId::try_from(event.product_id.as_str()).map_err(|source| {
        ProductEmbeddingWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;
    Ok(EmbedProductCommand {
        event_id,
        product_id,
    })
}

#[derive(Debug, thiserror::Error)]
enum ProductEmbeddingWorkerError {
    #[error("product embedding queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("product embedding job has an invalid event id")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("product embedding job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductEventJob, WorkerQueue};

    #[test]
    fn should_map_product_event_job_to_embedding_command() {
        let product_id = ProductId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductEmbed,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductEvent(ProductEventJob {
                event_id: event_id.to_string(),
                product_id: product_id.to_string(),
                event_type: "DOMAIN_CREATED".to_owned(),
                event_group: "DOMAIN".to_owned(),
            }),
        });
        assert!(
            matches!(command, Ok(EmbedProductCommand { event_id: actual_event_id, product_id: actual_product_id }) if actual_event_id == event_id && actual_product_id == product_id)
        );
    }
}
