use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_percolator_lambda::execute_job;
#[cfg(test)]
use search_filter_percolator_lambda::percolator_outcome;
pub use search_filter_percolator_lambda::{
    SearchFilterPercolatorJobDisposition, process_search_filter_percolator_job,
};
#[cfg(test)]
use search_filter_service::use_cases::MatchProductListingEventOutcome;
use search_filter_service::use_cases::MatchProductListingEventUseCase;
use std::sync::Arc;

pub async fn consume_search_filter_percolator_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn MatchProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterPercolator, move |job| {
            let use_case = Arc::clone(&use_case);
            async move { polling_outcome(execute_job(use_case.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: SearchFilterPercolatorJobDisposition) -> JobOutcome {
    match disposition {
        SearchFilterPercolatorJobDisposition::Complete(category) => JobOutcome::Complete(category),
        SearchFilterPercolatorJobDisposition::Retry(category) => JobOutcome::Retry(category),
        SearchFilterPercolatorJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        SearchFilterPercolatorJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_not_ack_missing_source_or_invalidate_historical_work_in_transport() {
        assert_eq!(
            SearchFilterPercolatorJobDisposition::Retry("missing_source"),
            percolator_outcome(MatchProductListingEventOutcome::SourceNotFound)
        );
        for outcome in [
            MatchProductListingEventOutcome::Processed,
            MatchProductListingEventOutcome::DuplicateAlreadyPersisted,
            MatchProductListingEventOutcome::StaleSourceSkipped,
            MatchProductListingEventOutcome::InactiveSourceSkipped,
            MatchProductListingEventOutcome::IgnoredEventType,
        ] {
            assert!(matches!(
                percolator_outcome(outcome),
                SearchFilterPercolatorJobDisposition::Complete(_)
            ));
        }
    }

    #[test]
    fn should_preserve_retry_dependency_and_poison_categories_for_each_transport() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            polling_outcome(SearchFilterPercolatorJobDisposition::Retry(
                "missing_source"
            ))
        );
        assert_eq!(
            JobOutcome::DependencyUnavailable("percolator_unavailable"),
            polling_outcome(SearchFilterPercolatorJobDisposition::DependencyUnavailable(
                "percolator_unavailable"
            ))
        );
        assert_eq!(
            JobOutcome::Invalid("invalid_wire_job"),
            polling_outcome(SearchFilterPercolatorJobDisposition::Poison(
                "invalid_wire_job"
            ))
        );
    }
}
