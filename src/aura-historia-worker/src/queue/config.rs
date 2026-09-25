#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

use crate::WorkerScope;
use platform_worker_queue_contract as contract;
use std::{ops::Deref, time::Duration};
use url::Url;

pub(super) use contract::Attributes;
pub use contract::{AWS_REGION_ENV, QueueError, SQS_ENDPOINT_ENV, WORKER_QUEUE_URL_ENV};

/// Native worker queue configuration, retaining its execution budget independently of the
/// deployed source-queue visibility shared with the router.
#[derive(Clone, Debug)]
pub struct SqsQueueConfig {
    pub(super) scope: WorkerScope,
    pub(super) queue_url: Url,
    pub(super) region: String,
    destination: contract::SqsQueueConfig,
}

impl From<contract::SqsQueueConfig> for SqsQueueConfig {
    fn from(destination: contract::SqsQueueConfig) -> Self {
        Self {
            scope: destination.scope(),
            queue_url: destination.queue_url().clone(),
            region: destination.region().to_owned(),
            destination,
        }
    }
}

impl Deref for SqsQueueConfig {
    type Target = contract::SqsQueueConfig;

    fn deref(&self) -> &Self::Target {
        &self.destination
    }
}

impl SqsQueueConfig {
    pub fn new(
        scope: WorkerScope,
        queue_url: Url,
        region: String,
        stage: String,
        local_endpoint: Option<Url>,
    ) -> Result<Self, QueueError> {
        contract::SqsQueueConfig::new(scope, queue_url, region, stage, local_endpoint)
            .map(Self::from)
    }

    pub fn from_env(scope: WorkerScope) -> Result<Self, QueueError> {
        contract::SqsQueueConfig::from_env(scope).map(Self::from)
    }

    pub(crate) fn from_getter<F>(scope: WorkerScope, get: F) -> Result<Self, QueueError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        contract::SqsQueueConfig::from_getter(scope, get).map(Self::from)
    }

    pub const fn scope(&self) -> WorkerScope {
        self.scope
    }

    /// Legacy/native-worker execution budget retained through consumer cutover.
    pub fn execution_budget(&self) -> Duration {
        execution_budget(self.scope())
    }
}

pub(super) fn visibility(scope: WorkerScope) -> Duration {
    contract::visibility(scope)
}

pub(super) fn execution_budget(scope: WorkerScope) -> Duration {
    // Notification's service-owned lease lasts five minutes. Leave finalization/drain headroom.
    Duration::from_secs(match scope {
        WorkerScope::NotificationDelivery
        | WorkerScope::SearchFilterPercolator
        | WorkerScope::ProductListingTranslation
        | WorkerScope::ProductListingEmbedding
        | WorkerScope::ProductListingOpenSearch
        | WorkerScope::ProductListingRawNormalization => 240,
        WorkerScope::SearchFilterProjection
        | WorkerScope::SearchFilterMatchNotification
        | WorkerScope::WatchlistNotification
        | WorkerScope::ProductListingContentAssessment => 45,
    })
}

/// The native router retains its worker-local queue type; every destination is validated by the
/// same read-only contract as the dedicated router Lambda.
#[derive(Clone, Debug)]
pub struct CdcRouterQueueConfig(contract::CdcRouterQueueConfig);

impl CdcRouterQueueConfig {
    pub fn from_env() -> Result<Self, QueueError> {
        contract::CdcRouterQueueConfig::from_env().map(Self)
    }

    #[cfg(test)]
    pub(crate) fn from_getter<F>(get: F) -> Result<Self, QueueError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        contract::CdcRouterQueueConfig::from_getter(get).map(Self)
    }

    pub fn region(&self) -> &str {
        self.0.region()
    }

    pub fn into_queues(self) -> Vec<SqsQueueConfig> {
        self.0.into_queues().into_iter().map(Into::into).collect()
    }
}

pub(super) fn validate_attributes(
    config: &SqsQueueConfig,
    attributes: &Attributes,
    dlq: bool,
) -> Result<(), QueueError> {
    contract::validate_attributes(&config.destination, attributes, dlq)
}
