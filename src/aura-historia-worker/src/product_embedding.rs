#[cfg(test)]
use crate::cdc::{DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_embedding_lambda::execute_job;
pub use product_embedding_lambda::{ProductEmbeddingJobDisposition, process_product_embedding_job};
#[cfg(test)]
use product_embedding_lambda::{command_from_job, embedding_disposition};
use product_listing_service::use_cases::EmbedProductListingEventUseCase;
#[cfg(test)]
use product_listing_service::use_cases::{
    EmbedProductListingCommand, EmbedProductListingEventOutcome,
};
use std::sync::Arc;

pub async fn consume_product_embedding_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn EmbedProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingEmbedding, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: ProductEmbeddingJobDisposition) -> JobOutcome {
    match disposition {
        ProductEmbeddingJobDisposition::Complete(category) => JobOutcome::Complete(category),
        ProductEmbeddingJobDisposition::Retry(category) => JobOutcome::Retry(category),
        ProductEmbeddingJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        ProductEmbeddingJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    #[test]
    fn should_map_product_event_job_to_embedding_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingEmbed,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(EmbedProductListingCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }
    #[test]
    fn should_complete_authoritative_missing_title_but_retain_missing_source() {
        assert_eq!(
            ProductEmbeddingJobDisposition::Complete("missing_title"),
            embedding_disposition(EmbedProductListingEventOutcome::MissingTitle)
        );
        assert_eq!(
            ProductEmbeddingJobDisposition::Retry("missing_source"),
            embedding_disposition(EmbedProductListingEventOutcome::ProductListingNotFound)
        );
    }

    #[test]
    fn should_preserve_retry_and_poison_dispositions_for_sqs_redrive() {
        assert_eq!(
            JobOutcome::DependencyUnavailable("embedding_unavailable"),
            polling_outcome(ProductEmbeddingJobDisposition::DependencyUnavailable(
                "embedding_unavailable"
            ))
        );
        assert_eq!(
            JobOutcome::Invalid("invalid_wire_job"),
            polling_outcome(ProductEmbeddingJobDisposition::Poison("invalid_wire_job"))
        );
    }
}
