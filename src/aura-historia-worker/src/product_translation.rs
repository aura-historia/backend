#[cfg(test)]
use crate::cdc::{DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::use_cases::TranslateProductListingEventUseCase;
#[cfg(test)]
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventOutcome,
};
use product_translation_lambda::execute_job;
pub use product_translation_lambda::{
    ProductTranslationJobDisposition, process_product_translation_job,
};
#[cfg(test)]
use product_translation_lambda::{command_from_job, translation_disposition};
use std::sync::Arc;

pub async fn consume_product_translation_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingTranslation, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: ProductTranslationJobDisposition) -> JobOutcome {
    match disposition {
        ProductTranslationJobDisposition::Complete(category) => JobOutcome::Complete(category),
        ProductTranslationJobDisposition::Retry(category) => JobOutcome::Retry(category),
        ProductTranslationJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        ProductTranslationJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    #[test]
    fn should_map_product_event_job_to_translation_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingTranslate,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(TranslateProductListingCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }
    #[test]
    fn should_complete_authoritative_noops_but_retain_missing_source() {
        assert_eq!(
            ProductTranslationJobDisposition::Complete("missing_title"),
            translation_disposition(TranslateProductListingEventOutcome::MissingTitle)
        );
        assert_eq!(
            ProductTranslationJobDisposition::Complete("missing_title_language"),
            translation_disposition(TranslateProductListingEventOutcome::MissingTitleLanguage)
        );
        assert_eq!(
            ProductTranslationJobDisposition::Complete("empty_title"),
            translation_disposition(TranslateProductListingEventOutcome::EmptyTitle)
        );
        assert_eq!(
            ProductTranslationJobDisposition::Retry("missing_source"),
            translation_disposition(TranslateProductListingEventOutcome::ProductListingNotFound)
        );
    }

    #[test]
    fn should_preserve_unfinished_dispositions_for_sqs_retry_or_redrive() {
        assert_eq!(
            JobOutcome::DependencyUnavailable("translation_unavailable"),
            polling_outcome(ProductTranslationJobDisposition::DependencyUnavailable(
                "translation_unavailable"
            ))
        );
        assert_eq!(
            JobOutcome::Invalid("invalid_wire_job"),
            polling_outcome(ProductTranslationJobDisposition::Poison("invalid_wire_job"))
        );
    }
}
