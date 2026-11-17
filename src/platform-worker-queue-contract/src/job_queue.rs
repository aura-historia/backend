//! The PostgreSQL job queue scopes that replace the SQS worker and command queues. Each scope
//! keeps its SQS queue's name, ordering, visibility (as the lease) and retry budget. A failed
//! SQS attempt became visible again once its visibility timeout passed, so the retry backoff
//! carries over as the same duration.

use crate::{MAX_RECEIVE_COUNT, visibility};
use aura_historia_jobs::{WorkerQueueType, WorkerScope};
use platform_postgres_queue::{JobScope, QueueOrdering, ScopeName};
use std::time::Duration;

/// The ProductListing ingestion command FIFO; not a CDC worker scope.
pub const PRODUCT_LISTING_INGESTION_SCOPE: &str = "product-listing-ingestion";
/// Visibility of `product-listing-ingestion-queue.fifo` in `infra/src/constructs/queues.ts`.
const PRODUCT_LISTING_INGESTION_VISIBILITY: Duration = Duration::from_secs(270);

/// The job queue scope that replaces a CDC worker scope's SQS queue.
pub fn worker_job_scope(scope: WorkerScope) -> JobScope {
    let ordering = match scope.queue_type() {
        WorkerQueueType::Standard => QueueOrdering::Standard,
        WorkerQueueType::Fifo => QueueOrdering::Fifo,
    };
    job_scope(scope.as_str(), ordering, visibility(scope))
}

/// The job queue scope that replaces the ProductListing ingestion command FIFO.
pub fn product_listing_ingestion_job_scope() -> JobScope {
    job_scope(
        PRODUCT_LISTING_INGESTION_SCOPE,
        QueueOrdering::Fifo,
        PRODUCT_LISTING_INGESTION_VISIBILITY,
    )
}

/// Every scope the job queue serves.
pub fn all_job_scopes() -> Vec<JobScope> {
    WorkerScope::ALL
        .into_iter()
        .map(worker_job_scope)
        .chain([product_listing_ingestion_job_scope()])
        .collect()
}

fn job_scope(name: &'static str, ordering: QueueOrdering, visibility: Duration) -> JobScope {
    // Every name and policy here is a constant this module's tests construct.
    JobScope::new(
        ScopeName::new(name).expect("catalogued job scope names are valid"),
        ordering,
        visibility,
        MAX_RECEIVE_COUNT,
        visibility,
    )
    .expect("catalogued job scope policies are valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn should_carry_each_worker_queue_name_ordering_visibility_and_retry_budget_over() {
        for scope in WorkerScope::ALL {
            let job_scope = worker_job_scope(scope);

            assert_eq!(scope.as_str(), job_scope.name().as_str());
            assert_eq!(visibility(scope), job_scope.lease());
            assert_eq!(visibility(scope), job_scope.retry_delay());
            assert_eq!(5, job_scope.max_attempts());
            let expected = match scope {
                WorkerScope::MarketingConsentSync => QueueOrdering::Fifo,
                _ => QueueOrdering::Standard,
            };
            assert_eq!(expected, job_scope.ordering(), "{}", scope.as_str());
        }
    }

    #[test]
    fn should_carry_the_ingestion_command_fifo_over() {
        let scope = product_listing_ingestion_job_scope();

        assert_eq!("product-listing-ingestion", scope.name().as_str());
        assert_eq!(QueueOrdering::Fifo, scope.ordering());
        assert_eq!(Duration::from_secs(270), scope.lease());
        assert_eq!(Duration::from_secs(270), scope.retry_delay());
        assert_eq!(5, scope.max_attempts());
    }

    #[test]
    fn should_catalogue_every_scope_once() {
        let scopes = all_job_scopes();
        let names: HashSet<_> = scopes.iter().map(|scope| scope.name().clone()).collect();

        assert_eq!(WorkerScope::ALL.len() + 1, scopes.len());
        assert_eq!(scopes.len(), names.len());
    }
}
