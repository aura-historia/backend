use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
    wire,
};
use search_filter_service::use_cases::{
    MatchProductListingEventCommand, MatchProductListingEventError,
    MatchProductListingEventOutcome, MatchProductListingEventUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for a fully handled saved-filter percolation job.
///
/// Only `Complete` may be acknowledged. PostgreSQL remains the authoritative match source;
/// missing committed sources and every unfinished/ambiguous failure stay on SQS for retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterPercolatorJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterPercolatorJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 saved-filter percolation job.
///
/// This contains no Lambda DTOs so native and Lambda transports retain their own delivery
/// lifecycles while using the exact same strict job and service-source guards.
pub async fn process_search_filter_percolator_job(
    body: &str,
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
) -> SearchFilterPercolatorJobDisposition {
    match wire::decode(body, WorkerScope::SearchFilterPercolator) {
        Ok(job) => execute_job(use_case, job).await,
        Err(_) => SearchFilterPercolatorJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    use_case: &(dyn MatchProductListingEventUseCase + Send + Sync),
    job: DomainJob,
) -> SearchFilterPercolatorJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return SearchFilterPercolatorJobDisposition::Poison("unexpected_payload");
    };
    match use_case
        .execute(MatchProductListingEventCommand {
            origin_event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => {
            tracing::info!(
                percolated_count = result.percolated_count,
                persisted_match_count = result.persisted_match_count,
                enhanced_evaluation_failure_count = result.enhanced_evaluation_failure_count,
                outcome = percolator_outcome(result.outcome).category(),
                "percolation completed"
            );
            percolator_outcome(result.outcome)
        }
        Err(error) => {
            use MatchProductListingEventError as E;
            match error {
                E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::SaleSnapshotStateInvalid { .. }
                | E::EventSnapshotStateInvalid { .. }
                | E::EventValuationConversionFailed { .. }
                | E::CandidateStateInvalid { .. }
                | E::PersistedMatchStateInvalid { .. } => {
                    SearchFilterPercolatorJobDisposition::Poison("percolator_state_invalid")
                }
                E::SaleSnapshotNotFound { .. } | E::EventSnapshotNotFound { .. } => {
                    SearchFilterPercolatorJobDisposition::Retry("valuation_snapshot_missing")
                }
                _ => SearchFilterPercolatorJobDisposition::DependencyUnavailable(
                    "percolator_unavailable",
                ),
            }
        }
    }
}

fn percolator_outcome(
    outcome: MatchProductListingEventOutcome,
) -> SearchFilterPercolatorJobDisposition {
    match outcome {
        MatchProductListingEventOutcome::Processed => {
            SearchFilterPercolatorJobDisposition::Complete("processed")
        }
        MatchProductListingEventOutcome::DuplicateAlreadyPersisted => {
            SearchFilterPercolatorJobDisposition::Complete("duplicate")
        }
        MatchProductListingEventOutcome::StaleSourceSkipped => {
            SearchFilterPercolatorJobDisposition::Complete("stale")
        }
        MatchProductListingEventOutcome::InactiveSourceSkipped => {
            SearchFilterPercolatorJobDisposition::Complete("inactive_source")
        }
        MatchProductListingEventOutcome::IgnoredEventType => {
            SearchFilterPercolatorJobDisposition::Complete("ignored_event")
        }
        MatchProductListingEventOutcome::SourceNotFound => {
            SearchFilterPercolatorJobDisposition::Retry("missing_source")
        }
    }
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
