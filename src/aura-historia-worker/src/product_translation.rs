use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventError,
    TranslateProductListingEventOutcome, TranslateProductListingEventUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for one translation job.
///
/// Only outcomes whose guarded PostgreSQL completion is known are acknowledged. A provider
/// response, including a response whose commit result was lost, never proves completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductTranslationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl ProductTranslationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 translation job.
///
/// The adapter provides the trusted system context only. The service owns source validation,
/// inference, and atomic source-version-guarded persistence.
pub async fn process_product_translation_job(
    body: &str,
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
) -> ProductTranslationJobDisposition {
    match crate::wire::decode(body, WorkerScope::ProductListingTranslation) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductTranslationJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn TranslateProductListingEventUseCase + Send + Sync),
    job: DomainJob,
) -> ProductTranslationJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return ProductTranslationJobDisposition::Poison("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-translation:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => translation_disposition(result.outcome),
        Err(TranslateProductListingEventError::ServiceOrSystemPrincipalRequired) => {
            ProductTranslationJobDisposition::Poison("system_principal_required")
        }
        // Source, provider, write, and commit failures retain the job. A completed provider call
        // is deliberately indistinguishable from unknown unfinished work until the guarded commit.
        Err(_) => {
            ProductTranslationJobDisposition::DependencyUnavailable("translation_unavailable")
        }
    }
}

fn translation_disposition(
    outcome: TranslateProductListingEventOutcome,
) -> ProductTranslationJobDisposition {
    use TranslateProductListingEventOutcome as O;
    match outcome {
        O::Applied => ProductTranslationJobDisposition::Complete("applied"),
        O::Duplicate => ProductTranslationJobDisposition::Complete("duplicate"),
        O::Stale => ProductTranslationJobDisposition::Complete("stale"),
        O::IgnoredEvent => ProductTranslationJobDisposition::Complete("ignored_event"),
        O::MissingTitle => ProductTranslationJobDisposition::Complete("missing_title"),
        O::MissingTitleLanguage => {
            ProductTranslationJobDisposition::Complete("missing_title_language")
        }
        O::EmptyTitle => ProductTranslationJobDisposition::Complete("empty_title"),
        O::ProductListingNotFound => ProductTranslationJobDisposition::Retry("missing_source"),
    }
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
fn command_from_job(
    job: DomainJob,
) -> Result<TranslateProductListingCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(TranslateProductListingCommand {
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
