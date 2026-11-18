//! The scopes this worker consumes and their per-scope defaults.

use aura_historia_jobs::WorkerScope;
use platform_postgres_queue::JobScope;
use platform_worker_queue_contract::job_queue::{
    PRODUCT_LISTING_INGESTION_SCOPE, ProjectionGeneration, ProjectionTarget,
    product_listing_ingestion_job_scope, worker_job_scope,
};
use std::time::Duration;

/// A scope with a consumer in this worker: every CDC worker scope, the ProductListing ingestion
/// command FIFO, and an optional isolated projection generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsumedScope {
    Worker(WorkerScope),
    ProductListingIngestion,
    ProjectionGeneration(ProjectionGeneration),
}

/// What a consumer starts from unless configuration overrides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeDefaults {
    /// Jobs run at once: the scope's production reserved Lambda concurrency, where each
    /// invocation processed its records one at a time.
    pub concurrency: u16,
    /// The most one attempt may run: the Lambda adapter's per-record budget.
    pub job_timeout: Duration,
}

impl ConsumedScope {
    /// Every scope a worker consumes by default.
    pub fn all() -> Vec<Self> {
        WorkerScope::ALL
            .into_iter()
            .map(Self::Worker)
            .chain([Self::ProductListingIngestion])
            .collect()
    }

    /// Parses a scope by its queue name, such as `geo-resolution` or
    /// `product-listing-opensearch-g7` for a projection generation.
    pub fn parse(name: &str) -> Option<Self> {
        if name == PRODUCT_LISTING_INGESTION_SCOPE {
            return Some(Self::ProductListingIngestion);
        }
        if let Some(scope) = WorkerScope::ALL
            .into_iter()
            .find(|scope| scope.as_str() == name)
        {
            return Some(Self::Worker(scope));
        }
        let generation = name
            .strip_prefix(WorkerScope::ProductListingOpenSearch.as_str())?
            .strip_prefix('-')?;
        ProjectionGeneration::new(generation)
            .ok()
            .map(Self::ProjectionGeneration)
    }

    pub fn job_scope(&self) -> JobScope {
        match self {
            Self::Worker(scope) => worker_job_scope(*scope),
            Self::ProductListingIngestion => product_listing_ingestion_job_scope(),
            Self::ProjectionGeneration(generation) => {
                ProjectionTarget::Generation(generation.clone()).job_scope()
            }
        }
    }

    pub fn defaults(&self) -> ScopeDefaults {
        let (concurrency, job_timeout_seconds) = match self {
            Self::Worker(scope) => match scope {
                WorkerScope::SearchFilterMatchNotification => (5, 40),
                WorkerScope::WatchlistNotification => (5, 40),
                WorkerScope::ProductListingContentAssessment => (5, 40),
                WorkerScope::ProductListingTranslation => (15, 40),
                WorkerScope::ProductListingEmbedding => (10, 55),
                WorkerScope::ProductListingOpenSearch => (10, 40),
                WorkerScope::ProductListingRawNormalization => (5, 40),
                WorkerScope::NotificationDelivery => (5, 40),
                WorkerScope::MarketingConsentSync => (2, 35),
                WorkerScope::GeoResolution => (2, 25),
                WorkerScope::ListingGeography => (2, 25),
                WorkerScope::ListingShipping => (2, 25),
                WorkerScope::UserIdentityDeletion => (2, 25),
            },
            Self::ProductListingIngestion => (2, 35),
            // A rebuild generation shares the live projector's policy.
            Self::ProjectionGeneration(_) => (10, 40),
        };
        ScopeDefaults {
            concurrency,
            job_timeout: Duration::from_secs(job_timeout_seconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_every_consumed_scope_by_its_queue_name() {
        for scope in ConsumedScope::all() {
            let name = scope.job_scope().name().as_str().to_owned();
            assert_eq!(Some(scope), ConsumedScope::parse(&name), "{name}");
        }
        assert_eq!(
            Some(ConsumedScope::ProjectionGeneration(
                ProjectionGeneration::new("g7").unwrap()
            )),
            ConsumedScope::parse("product-listing-opensearch-g7")
        );
        for unknown in [
            "",
            "unknown",
            "product-listing-opensearch-",
            "product-listing-opensearch-G7",
        ] {
            assert_eq!(None, ConsumedScope::parse(unknown), "{unknown}");
        }
    }

    #[test]
    fn should_end_every_default_job_timeout_within_its_lease() {
        let generation =
            ConsumedScope::ProjectionGeneration(ProjectionGeneration::new("g1").unwrap());
        for scope in ConsumedScope::all().into_iter().chain([generation]) {
            let defaults = scope.defaults();
            assert!(defaults.concurrency > 0);
            assert!(
                defaults.job_timeout < scope.job_scope().lease(),
                "{scope:?}"
            );
        }
    }
}
