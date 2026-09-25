use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_opensearch_lambda::execute_job;
#[cfg(test)]
use product_listing_opensearch_lambda::projection_disposition;
pub use product_listing_opensearch_lambda::{
    ProductListingOpenSearchJobDisposition, process_product_listing_opensearch_job,
};
#[cfg(test)]
use product_listing_service::use_cases::ProjectProductListingOutcome;
use product_listing_service::use_cases::ProjectProductListingUseCase;
use std::sync::Arc;

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

fn polling_outcome(disposition: ProductListingOpenSearchJobDisposition) -> JobOutcome {
    match disposition {
        ProductListingOpenSearchJobDisposition::Complete(category) => {
            JobOutcome::Complete(category)
        }
        ProductListingOpenSearchJobDisposition::Retry(category) => JobOutcome::Retry(category),
        ProductListingOpenSearchJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
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
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            polling_outcome(projection_disposition(
                ProjectProductListingOutcome::MissingSource
            ))
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

    #[test]
    fn should_preserve_projection_dependency_failures_for_native_consumer_circuit_breaking() {
        assert_eq!(
            JobOutcome::DependencyUnavailable("projection_unavailable"),
            polling_outcome(
                ProductListingOpenSearchJobDisposition::DependencyUnavailable(
                    "projection_unavailable"
                )
            )
        );
    }
}
