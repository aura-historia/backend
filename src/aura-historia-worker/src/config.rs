//! Worker configuration from the environment.

use crate::scope::ConsumedScope;
use platform_process::config::{self, ConfigError};
use platform_worker::{ConsumerConfig, ConsumerConfigError};
use platform_worker_queue_contract::job_queue::ProjectionGeneration;
use std::{num::NonZeroU16, path::PathBuf, time::Duration};

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
/// The CloudWatch namespace of the host health metrics; unset, the worker publishes none.
pub const METRICS_NAMESPACE_ENV: &str = "AURA_HISTORIA_WORKER_METRICS_NAMESPACE";
/// A file whose modification time is the last successful database backup.
pub const METRICS_BACKUP_MARKER_ENV: &str = "AURA_HISTORIA_WORKER_METRICS_BACKUP_MARKER";
/// A path on the host filesystem whose free space the metrics report.
pub const METRICS_DISK_PATH_ENV: &str = "AURA_HISTORIA_WORKER_METRICS_DISK_PATH";
/// The directory whose files the host's scheduled jobs touch after each success, one per job.
pub const METRICS_JOB_MARKERS_ENV: &str = "AURA_HISTORIA_WORKER_METRICS_JOB_MARKERS";

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(25);
const DEFAULT_DISPATCH_BATCH_SIZE: u16 = 100;
/// Connections beyond the jobs' own: the two wake-up listeners, the dispatcher, settlement and
/// the health metrics.
const POOL_HEADROOM: u32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum WorkerConfigError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("unknown worker scope in {SCOPES_ENV}")]
    UnknownScope,
    #[error("{SCOPES_ENV} and {PROJECTION_GENERATIONS_ENV} select no scope")]
    NoScope,
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
    /// Where the host health metrics go, or `None` when this process publishes none.
    pub health_metrics: Option<HealthMetricsConfig>,
}

/// The host health metrics' destination and the host paths they read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthMetricsConfig {
    pub namespace: String,
    pub backup_marker: PathBuf,
    pub disk_path: PathBuf,
    pub job_markers: PathBuf,
}

impl HealthMetricsConfig {
    /// Configured by its namespace; the paths are then required.
    fn from_env() -> Result<Option<Self>, ConfigError> {
        let namespace = config::optional(METRICS_NAMESPACE_ENV, String::new())?;
        if namespace.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            namespace,
            backup_marker: config::required(METRICS_BACKUP_MARKER_ENV)?.into(),
            disk_path: config::required(METRICS_DISK_PATH_ENV)?.into(),
            job_markers: config::required(METRICS_JOB_MARKERS_ENV)?.into(),
        }))
    }
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
            health_metrics: HealthMetricsConfig::from_env()?,
        })
    }

    /// The fixed pool this process needs: one connection per concurrent job, as each Lambda
    /// invocation had, plus the listeners, the dispatcher, settlement and the health metrics.
    pub fn pool_size(&self) -> u32 {
        self.scopes
            .iter()
            .map(|scope| u32::from(scope.consumer.concurrency().get()))
            .sum::<u32>()
            + POOL_HEADROOM
    }
}

fn selected_scopes() -> Result<Vec<ConsumedScope>, WorkerConfigError> {
    parse_scopes(
        &config::optional(SCOPES_ENV, "all".to_owned())?,
        &config::optional(PROJECTION_GENERATIONS_ENV, String::new())?,
    )
}

/// The scopes `selected` (`all` or a comma-separated list) and the projection `generations` name,
/// each once.
fn parse_scopes(
    selected: &str,
    generations: &str,
) -> Result<Vec<ConsumedScope>, WorkerConfigError> {
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
    // A blank selection would start a worker that consumes nothing while jobs wait.
    if unique.is_empty() {
        return Err(WorkerConfigError::NoScope);
    }
    Ok(unique)
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

    #[test]
    fn should_refuse_a_selection_without_any_scope() {
        for (selected, generations) in [("", ""), (" , ", ""), ("", " ")] {
            assert!(
                matches!(
                    parse_scopes(selected, generations),
                    Err(WorkerConfigError::NoScope)
                ),
                "{selected:?} / {generations:?}"
            );
        }
    }

    #[test]
    fn should_consume_each_selected_scope_and_generation_once() {
        let scopes = parse_scopes("geo-resolution, geo-resolution", "g2").unwrap();

        assert_eq!(2, scopes.len());
        assert_eq!(ConsumedScope::Worker(WorkerScope::GeoResolution), scopes[0]);
        assert!(matches!(scopes[1], ConsumedScope::ProjectionGeneration(_)));
        assert_eq!(
            ConsumedScope::all().len(),
            parse_scopes("all", "").unwrap().len()
        );
    }
}
