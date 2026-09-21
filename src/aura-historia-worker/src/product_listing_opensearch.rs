use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
    wire,
};
use product_listing_service::use_cases::{
    ProjectProductListingCommand, ProjectProductListingError, ProjectProductListingOutcome,
    ProjectProductListingUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for a fully handled ProductListing projection job.
///
/// Only `Complete` may be acknowledged. `Retry` and `Poison` deliberately remain
/// on the source queue so native SQS retry/redrive owns recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingOpenSearchJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    Poison(&'static str),
}

impl ProductListingOpenSearchJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category) | Self::Retry(category) | Self::Poison(category) => category,
        }
    }
}

pub async fn consume_product_listing_opensearch_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn ProjectProductListingUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingOpenSearch, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

/// Decode and execute one compact schema-2 ProductListing OpenSearch job.
///
/// This is deliberately free of Lambda/SQS DTOs so each transport can retain its
/// own acknowledgment lifecycle while sharing the same strict wire and service rules.
pub async fn process_product_listing_opensearch_job(
    body: &str,
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
) -> ProductListingOpenSearchJobDisposition {
    match wire::decode(body, WorkerScope::ProductListingOpenSearch) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => ProductListingOpenSearchJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn ProjectProductListingUseCase + Send + Sync),
    job: DomainJob,
) -> ProductListingOpenSearchJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return ProductListingOpenSearchJobDisposition::Poison("unexpected_payload");
    };
    match use_case
        .execute(ProjectProductListingCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => projection_disposition(result.outcome),
        Err(ProjectProductListingError::SaleObservationFxSnapshotMissing) => {
            ProductListingOpenSearchJobDisposition::Retry("sale_snapshot_missing")
        }
        Err(ProjectProductListingError::SaleObservationFxSnapshotInvalid { .. }) => {
            ProductListingOpenSearchJobDisposition::Poison("sale_snapshot_invalid")
        }
        Err(_) => ProductListingOpenSearchJobDisposition::Retry("projection_unavailable"),
    }
}

fn projection_disposition(
    outcome: ProjectProductListingOutcome,
) -> ProductListingOpenSearchJobDisposition {
    match outcome {
        ProjectProductListingOutcome::Applied => {
            ProductListingOpenSearchJobDisposition::Complete("applied")
        }
        ProjectProductListingOutcome::Deleted => {
            ProductListingOpenSearchJobDisposition::Complete("deleted")
        }
        ProjectProductListingOutcome::Stale => {
            ProductListingOpenSearchJobDisposition::Complete("stale")
        }
        // Absence of the committed event/source is not evidence that its projection was removed.
        ProjectProductListingOutcome::MissingSource => {
            ProductListingOpenSearchJobDisposition::Retry("missing_source")
        }
    }
}

fn polling_outcome(disposition: ProductListingOpenSearchJobDisposition) -> JobOutcome {
    match disposition {
        ProductListingOpenSearchJobDisposition::Complete(category) => {
            JobOutcome::Complete(category)
        }
        ProductListingOpenSearchJobDisposition::Retry(category) => JobOutcome::Retry(category),
        ProductListingOpenSearchJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_retain_missing_source_but_complete_guarded_projection_outcomes() {
        assert_eq!(
            ProductListingOpenSearchJobDisposition::Retry("missing_source"),
            projection_disposition(ProjectProductListingOutcome::MissingSource)
        );
        for outcome in [
            ProjectProductListingOutcome::Applied,
            ProjectProductListingOutcome::Deleted,
            ProjectProductListingOutcome::Stale,
        ] {
            assert!(matches!(
                projection_disposition(outcome),
                ProductListingOpenSearchJobDisposition::Complete(_)
            ));
        }
    }
}
