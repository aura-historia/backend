//! Worker configuration from the environment.

use crate::scope::ConsumedScope;
use platform_process::config::{self, ConfigError};
use platform_worker::{ConsumerConfig, ConsumerConfigError};
use platform_worker_queue_contract::job_queue::ProjectionGeneration;
use std::{num::NonZeroU16, time::Duration};

/// Comma-separated scope names this worker consumes, or `all` (the default).
pub const SCOPES_ENV: &str = "AURA_HISTORIA_WORKER_SCOPES";
/// Comma-separated isolated projection generations to consume in addition, such as `g7`.
pub const PROJECTION_GENERATIONS_ENV: &str = "AURA_HISTORIA_WORKER_PROJECTION_GENERATIONS";
/// `false` runs no outbox dispatcher in this process.
pub const DISPATCHER_ENV: &str = "AURA_HISTORIA_WORKER_DISPATCHER";
pub const DISPATCH_BATCH_SIZE_ENV: &str = "AURA_HISTORIA_WORKER_DISPATCH_BATCH_SIZE";
pub const POLL_INTERVAL_MS_ENV: &str = "AURA_HISTORIA_WORKER_POLL_INTERVAL_MS";
pub const SHUTDOWN_GRACE_SECONDS_ENV: &str = "AURA_HISTORIA_WORKER_SHUTDOWN_GRACE_SECONDS";
/// Prefix of a per-scope concurrency override, followed by the scope name in upper snake case,
/// for example `AURA_HISTORIA_WORKER_CONCURRENCY_GEO_RESOLUTION`.
pub const CONCURRENCY_ENV_PREFIX: &str = "AURA_HISTORIA_WORKER_CONCURRENCY_";

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(25);
const DEFAULT_DISPATCH_BATCH_SIZE: u16 = 100;
/// Connections beyond the jobs' own: the two wake-up listeners, the dispatcher and settlement.
const POOL_HEADROOM: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum WorkerConfigError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unknown worker scope in {SCOPES_ENV}")]
    UnknownScope,
    #[error("invalid projection generation in {PROJECTION_GENERATIONS_ENV}")]
    InvalidGeneration,
    #[error("invalid consumer configuration")]
    Consumer(#[from] ConsumerConfigError),
}

/// One scope's consumer settings.
#[derive(Clone, Debug)]
pub struct ScopeConfig {
    pub scope: ConsumedScope,
    pub consumer: ConsumerConfig,
}

#[derive(Clone, Debug)]
pub struct WorkerConfig {
    pub scopes: Vec<ScopeConfig>,
    /// The outbox dispatch batch size, or `None` when this process runs no dispatcher.
    pub dispatch_batch_size: Option<NonZeroU16>,
    pub poll_interval: Duration,
    pub shutdown_grace: Duration,
}

impl WorkerConfig {
    pub fn from_env() -> Result<Self, WorkerConfigError> {
        let poll_interval = Duration::from_millis(config::optional(
            POLL_INTERVAL_MS_ENV,
            u64::try_from(DEFAULT_POLL_INTERVAL.as_millis()).unwrap_or(u64::MAX),
        )?);
        let shutdown_grace = Duration::from_secs(config::optional(
            SHUTDOWN_GRACE_SECONDS_ENV,
            DEFAULT_SHUTDOWN_GRACE.as_secs(),
        )?);
        let mut scopes = Vec::new();
        for scope in selected_scopes()? {
            let defaults = scope.defaults();
            let concurrency: NonZeroU16 = config::optional(
                &concurrency_env(&scope),
                NonZeroU16::new(defaults.concurrency).expect("default concurrency is positive"),
            )?;
            let consumer = ConsumerConfig::new(
                &scope.job_scope(),
                concurrency,
                defaults.job_timeout,
                poll_interval,
                shutdown_grace,
            )?;
            scopes.push(ScopeConfig { scope, consumer });
        }
        let dispatch_batch_size = config::optional(DISPATCHER_ENV, true)?
            .then(|| {
                config::optional(
                    DISPATCH_BATCH_SIZE_ENV,
                    NonZeroU16::new(DEFAULT_DISPATCH_BATCH_SIZE).expect("positive"),
                )
            })
            .transpose()?;
        Ok(Self {
            scopes,
            dispatch_batch_size,
            poll_interval,
            shutdown_grace,
        })
    }

    /// The fixed pool this process needs: one connection per concurrent job, as each Lambda
    /// invocation had, plus the listeners, the dispatcher and settlement.
    pub fn pool_size(&self) -> u32 {
        self.scopes
            .iter()
            .map(|scope| u32::from(scope.consumer.concurrency().get()))
            .sum::<u32>()
            + POOL_HEADROOM
    }
}

fn selected_scopes() -> Result<Vec<ConsumedScope>, WorkerConfigError> {
    let selected = config::optional(SCOPES_ENV, "all".to_owned())?;
    let mut scopes = if selected.trim() == "all" {
        ConsumedScope::all()
    } else {
        selected
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| ConsumedScope::parse(name).ok_or(WorkerConfigError::UnknownScope))
            .collect::<Result<Vec<_>, _>>()?
    };
    let generations = config::optional(PROJECTION_GENERATIONS_ENV, String::new())?;
    for generation in generations
        .split(',')
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        let generation = ProjectionGeneration::new(generation)
            .map_err(|_| WorkerConfigError::InvalidGeneration)?;
        scopes.push(ConsumedScope::ProjectionGeneration(generation));
    }
    let mut unique = Vec::with_capacity(scopes.len());
    for scope in scopes {
        if !unique.contains(&scope) {
            unique.push(scope);
        }
    }
    let scopes = unique;
    Ok(scopes)
}

fn concurrency_env(scope: &ConsumedScope) -> String {
    format!(
        "{CONCURRENCY_ENV_PREFIX}{}",
        scope
            .job_scope()
            .name()
            .as_str()
            .replace('-', "_")
            .to_ascii_uppercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_historia_jobs::WorkerScope;

    #[test]
    fn should_name_concurrency_overrides_after_the_scope() {
        assert_eq!(
            "AURA_HISTORIA_WORKER_CONCURRENCY_GEO_RESOLUTION",
            concurrency_env(&ConsumedScope::Worker(WorkerScope::GeoResolution))
        );
        assert_eq!(
            "AURA_HISTORIA_WORKER_CONCURRENCY_PRODUCT_LISTING_INGESTION",
            concurrency_env(&ConsumedScope::ProductListingIngestion)
        );
    }
}
