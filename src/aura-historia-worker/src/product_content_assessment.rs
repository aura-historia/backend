use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
    wire,
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    AssessProductListingContentCommand, AssessProductListingContentEventError,
    AssessProductListingContentEventOutcome, AssessProductListingContentEventUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for one content-assessment job.
///
/// Only a committed service outcome may be acknowledged. Missing committed sources,
/// transaction failures, timeouts, and malformed jobs remain on SQS for retry or redrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductContentAssessmentJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl ProductContentAssessmentJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 content-assessment job.
///
/// This adapter deliberately does not contain content-policy behavior; it only supplies the
/// trusted system operation context and preserves service-owned guarded persistence semantics.
pub async fn process_product_content_assessment_job(
    body: &str,
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
) -> ProductContentAssessmentJobDisposition {
    match wire::decode(body, WorkerScope::ProductListingContentAssessment) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductContentAssessmentJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn AssessProductListingContentEventUseCase + Send + Sync),
    job: DomainJob,
) -> ProductContentAssessmentJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductContentAssessmentJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-content-assessment:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => assessment_disposition(result.outcome),
        Err(AssessProductListingContentEventError::ServiceOrSystemPrincipalRequired) => {
            ProductContentAssessmentJobDisposition::Poison("system_principal_required")
        }
        // A provider is intentionally not part of this scope. Source, write, commit, and
        // unknown completion errors must remain retryable rather than being acknowledged.
        Err(_) => {
            ProductContentAssessmentJobDisposition::DependencyUnavailable("assessment_unavailable")
        }
    }
}

fn assessment_disposition(
    outcome: AssessProductListingContentEventOutcome,
) -> ProductContentAssessmentJobDisposition {
    use AssessProductListingContentEventOutcome as O;
    match outcome {
        O::Applied => ProductContentAssessmentJobDisposition::Complete("applied"),
        O::Cleared => ProductContentAssessmentJobDisposition::Complete("cleared"),
        O::Duplicate => ProductContentAssessmentJobDisposition::Complete("duplicate"),
        O::Stale => ProductContentAssessmentJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductContentAssessmentJobDisposition::Complete("ignored_event"),
        O::ProductListingNotFound => {
            ProductContentAssessmentJobDisposition::Retry("missing_source")
        }
    }
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

fn command_from_job(
    job: DomainJob,
) -> Result<AssessProductListingContentCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(AssessProductListingContentCommand {
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
