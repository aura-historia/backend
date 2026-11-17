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

/// Where a full ProductListing reprojection sends its projector jobs.
///
/// The live projector consumes `product-listing-opensearch` and writes the index that readers
/// serve. A separate generation has its own scope, `product-listing-opensearch-<generation>`,
/// whose projector consumer writes only the index `product-listings-<generation>`; the live
/// index never sees its jobs. Targeting the live index is an explicit choice, used to rebuild
/// search after the index was lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionTarget {
    Live,
    Generation(ProjectionGeneration),
}

impl ProjectionTarget {
    pub fn job_scope(&self) -> JobScope {
        let live = worker_job_scope(WorkerScope::ProductListingOpenSearch);
        match self {
            Self::Live => live,
            Self::Generation(generation) => JobScope::new(
                generation.scope_name(),
                live.ordering(),
                live.lease(),
                live.max_attempts(),
                live.retry_delay(),
            )
            .expect("the live projector policy is valid"),
        }
    }
}

const MAX_GENERATION_BYTES: usize = 16;

/// An isolated OpenSearch index generation: 1 to 16 lower-case ASCII letters or digits, so its
/// scope name stays a valid job scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionGeneration(String);

impl ProjectionGeneration {
    pub fn new(generation: impl Into<String>) -> Result<Self, InvalidProjectionGeneration> {
        let generation = generation.into();
        let valid = !generation.is_empty()
            && generation.len() <= MAX_GENERATION_BYTES
            && generation
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        valid
            .then_some(Self(generation))
            .ok_or(InvalidProjectionGeneration)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The index this generation's projector consumer writes.
    pub fn index_name(&self) -> String {
        format!("product-listings-{}", self.0)
    }

    fn scope_name(&self) -> ScopeName {
        ScopeName::new(format!(
            "{}-{}",
            WorkerScope::ProductListingOpenSearch.as_str(),
            self.0
        ))
        .expect("a validated generation yields a valid scope name")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("projection generation must be 1 to 16 lower-case ASCII letters or digits")]
pub struct InvalidProjectionGeneration;

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
    fn should_send_live_reprojection_to_the_live_projector_scope() {
        assert_eq!(
            worker_job_scope(WorkerScope::ProductListingOpenSearch),
            ProjectionTarget::Live.job_scope()
        );
    }

    #[test]
    fn should_isolate_a_generation_in_its_own_scope_with_the_live_policy() {
        let generation = ProjectionGeneration::new("g7").unwrap();
        let live = ProjectionTarget::Live.job_scope();

        let scope = ProjectionTarget::Generation(generation.clone()).job_scope();

        assert_eq!("product-listing-opensearch-g7", scope.name().as_str());
        assert_eq!("product-listings-g7", generation.index_name());
        assert_eq!(
            (
                live.ordering(),
                live.lease(),
                live.max_attempts(),
                live.retry_delay()
            ),
            (
                scope.ordering(),
                scope.lease(),
                scope.max_attempts(),
                scope.retry_delay()
            )
        );
    }

    #[test]
    fn should_accept_only_short_lower_case_alphanumeric_generations() {
        for generation in ["a", "g7", "2026", &"z".repeat(16)] {
            let generation = ProjectionGeneration::new(generation).unwrap();
            assert!(
                ProjectionTarget::Generation(generation)
                    .job_scope()
                    .name()
                    .as_str()
                    .len()
                    <= 48
            );
        }
        for generation in ["", "G7", "g-7", "g_7", "g 7", &"z".repeat(17)] {
            assert_eq!(
                Err(InvalidProjectionGeneration),
                ProjectionGeneration::new(generation),
                "{generation}"
            );
        }
    }

    #[test]
    fn should_catalogue_every_scope_once() {
        let scopes = all_job_scopes();
        let names: HashSet<_> = scopes.iter().map(|scope| scope.name().clone()).collect();

        assert_eq!(WorkerScope::ALL.len() + 1, scopes.len());
        assert_eq!(scopes.len(), names.len());
    }
}
