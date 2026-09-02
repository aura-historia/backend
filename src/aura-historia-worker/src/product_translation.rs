use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::{
    error::{BoxError, box_error},
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
};
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventOutcome,
    TranslateProductListingEventUseCase,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info};

pub async fn consume_product_translation_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
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
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                ?outcome,
                "product translation job completed"
            ),
            (Ok(()), None) => error!(
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                outcome = "missing",
                "product translation job completed without an outcome"
            ),
            (Err(error), _) => error!(
                job_type = "product_translation",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "product translation job failed"
            ),
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
    job: DomainJob,
    outcome: Arc<Mutex<Option<TranslateProductListingEventOutcome>>>,
) -> Result<(), BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-translation:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    let result = use_case
        .execute(&context, command)
        .await
        .map_err(box_error)?;
    *outcome.lock().await = Some(result.outcome);
    Ok(())
}

fn command_from_job(
    job: DomainJob,
) -> Result<TranslateProductListingCommand, ProductListingTranslationWorkerError> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(ProductListingTranslationWorkerError::UnexpectedJobPayload);
    };
    Ok(TranslateProductListingCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

#[derive(Debug, thiserror::Error)]
enum ProductListingTranslationWorkerError {
    #[error("product translation queue received an unexpected job payload")]
    UnexpectedJobPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;

    fn job(event_id: EventId, product_listing_id: ProductListingId) -> DomainJob {
        DomainJob {
            target_queue: WorkerQueue::ProductListingTranslate,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        }
    }

    #[test]
    fn should_map_product_event_job_to_translation_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();

        let command = command_from_job(job(event_id, product_listing_id));

        assert!(matches!(
            command,
            Ok(TranslateProductListingCommand { event_id: actual_event_id, product_listing_id: actual_product_listing_id })
                if actual_event_id == event_id && actual_product_listing_id == product_listing_id
        ));
    }
}
