#[cfg(test)]
use crate::cdc::{DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_content_assessment_lambda::execute_job;
pub use product_content_assessment_lambda::{
    ProductContentAssessmentJobDisposition, process_product_content_assessment_job,
};
#[cfg(test)]
use product_content_assessment_lambda::{assessment_disposition, command_from_job};
use product_listing_service::use_cases::AssessProductListingContentEventUseCase;
#[cfg(test)]
use product_listing_service::use_cases::{
    AssessProductListingContentCommand, AssessProductListingContentEventOutcome,
};
use std::sync::Arc;

pub async fn consume_product_content_assessment_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn AssessProductListingContentEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingContentAssessment, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: ProductContentAssessmentJobDisposition) -> JobOutcome {
    match disposition {
        ProductContentAssessmentJobDisposition::Complete(category) => {
            JobOutcome::Complete(category)
        }
        ProductContentAssessmentJobDisposition::Retry(category) => JobOutcome::Retry(category),
        ProductContentAssessmentJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        ProductContentAssessmentJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;

    #[test]
    fn should_map_product_event_job_to_content_assessment_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingContentAssessment,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(AssessProductListingContentCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }

    #[test]
    fn should_acknowledge_only_committed_assessment_outcomes() {
        for outcome in [
            AssessProductListingContentEventOutcome::Applied,
            AssessProductListingContentEventOutcome::Cleared,
            AssessProductListingContentEventOutcome::Duplicate,
            AssessProductListingContentEventOutcome::Stale,
            AssessProductListingContentEventOutcome::IgnoredEvent,
        ] {
            assert!(matches!(
                assessment_disposition(outcome),
                ProductContentAssessmentJobDisposition::Complete(_)
            ));
        }
        assert_eq!(
            ProductContentAssessmentJobDisposition::Retry("missing_source"),
            assessment_disposition(AssessProductListingContentEventOutcome::ProductListingNotFound)
        );
    }

    #[test]
    fn should_preserve_retry_and_poison_dispositions_for_sqs_redrive() {
        assert_eq!(
            JobOutcome::DependencyUnavailable("assessment_unavailable"),
            polling_outcome(
                ProductContentAssessmentJobDisposition::DependencyUnavailable(
                    "assessment_unavailable"
                )
            )
        );
        assert_eq!(
            JobOutcome::Invalid("invalid_wire_job"),
            polling_outcome(ProductContentAssessmentJobDisposition::Poison(
                "invalid_wire_job"
            ))
        );
    }
}
