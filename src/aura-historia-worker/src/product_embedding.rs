use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
    wire,
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    EmbedProductListingCommand, EmbedProductListingEventError, EmbedProductListingEventOutcome,
    EmbedProductListingEventUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for one embedding job.
///
/// Only outcomes whose source guard and persistence are known to be complete are acknowledged.
/// Source absence, provider failures, and an ambiguous transaction completion remain on SQS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEmbeddingJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl ProductEmbeddingJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 embedding job.
///
/// The adapter provides the trusted system operation context only. The service owns source
/// validation, provider invocation, and source-version-guarded persistence.
pub async fn process_product_embedding_job(
    body: &str,
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
) -> ProductEmbeddingJobDisposition {
    match wire::decode(body, WorkerScope::ProductListingEmbedding) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductEmbeddingJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn EmbedProductListingEventUseCase + Send + Sync),
    job: DomainJob,
) -> ProductEmbeddingJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductEmbeddingJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-embedding:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => embedding_disposition(result.outcome),
        Err(EmbedProductListingEventError::ServiceOrSystemPrincipalRequired) => {
            ProductEmbeddingJobDisposition::Poison("system_principal_required")
        }
        Err(EmbedProductListingEventError::InvalidInput { .. }) => {
            ProductEmbeddingJobDisposition::Poison("embedding_input_invalid")
        }
        // Provider, source, write, and commit failures leave the job retryable. In particular,
        // a provider result does not prove that the guarded PostgreSQL commit completed.
        Err(_) => ProductEmbeddingJobDisposition::DependencyUnavailable("embedding_unavailable"),
    }
}

fn embedding_disposition(
    outcome: EmbedProductListingEventOutcome,
) -> ProductEmbeddingJobDisposition {
    use EmbedProductListingEventOutcome as O;
    match outcome {
        O::Applied => ProductEmbeddingJobDisposition::Complete("applied"),
        O::Duplicate => ProductEmbeddingJobDisposition::Complete("duplicate"),
        O::Stale => ProductEmbeddingJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductEmbeddingJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductEmbeddingJobDisposition::Complete("missing_title"),
        O::ProductListingNotFound => ProductEmbeddingJobDisposition::Retry("missing_source"),
    }
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
fn command_from_job(job: DomainJob) -> Result<EmbedProductListingCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(EmbedProductListingCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
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
