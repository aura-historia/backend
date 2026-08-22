use std::collections::HashSet;
use std::net::{AddrParseError, SocketAddr};
use std::time::Duration;

pub const CRON_HEALTH_BIND_ADDR_ENV: &str = "AURA_HISTORIA_CRON_HEALTH_BIND_ADDR";
pub const CRON_SHUTDOWN_GRACE_SECONDS_ENV: &str = "AURA_HISTORIA_CRON_SHUTDOWN_GRACE_SECONDS";
pub const CRON_ENABLED_JOBS_ENV: &str = "AURA_HISTORIA_CRON_ENABLED_JOBS";
pub const STAGE_ENV: &str = "STAGE";

const DEFAULT_HEALTH_BIND_ADDR: &str = "0.0.0.0:8082";
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronRuntimeConfig {
    health_bind_addr: SocketAddr,
    shutdown_grace: Duration,
    enabled_jobs: Vec<String>,
}

impl CronRuntimeConfig {
    pub fn from_env(known_jobs: &[&str]) -> Result<Self, CronRuntimeConfigError> {
        Self::from_getter(|name| std::env::var(name).ok(), known_jobs)
    }

    pub(crate) fn from_getter<F>(
        mut get: F,
        known_jobs: &[&str],
    ) -> Result<Self, CronRuntimeConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let health_bind_addr_raw =
            get(CRON_HEALTH_BIND_ADDR_ENV).unwrap_or_else(|| DEFAULT_HEALTH_BIND_ADDR.to_owned());
        let health_bind_addr = health_bind_addr_raw.parse().map_err(|source| {
            CronRuntimeConfigError::InvalidHealthBindAddr {
                value: health_bind_addr_raw,
                source,
            }
        })?;
        let shutdown_grace_seconds = parse_positive_u64(
            get(CRON_SHUTDOWN_GRACE_SECONDS_ENV),
            CRON_SHUTDOWN_GRACE_SECONDS_ENV,
            DEFAULT_SHUTDOWN_GRACE_SECONDS,
        )?;
        let stage = get(STAGE_ENV);
        let enabled_jobs = parse_enabled_jobs(get(CRON_ENABLED_JOBS_ENV), known_jobs)?;
        if enabled_jobs.is_empty() && !is_local_stage(stage.as_deref()) {
            return Err(CronRuntimeConfigError::NoEnabledJobs);
        }

        Ok(Self {
            health_bind_addr,
            shutdown_grace: Duration::from_secs(shutdown_grace_seconds),
            enabled_jobs,
        })
    }

    pub const fn health_bind_addr(&self) -> SocketAddr {
        self.health_bind_addr
    }
    pub const fn shutdown_grace(&self) -> Duration {
        self.shutdown_grace
    }
    pub fn enabled_jobs(&self) -> &[String] {
        &self.enabled_jobs
    }
}

fn is_local_stage(stage: Option<&str>) -> bool {
    matches!(stage, Some("ephemeral" | "local" | "test"))
}

fn parse_positive_u64(
    value: Option<String>,
    name: &'static str,
    default: u64,
) -> Result<u64, CronRuntimeConfigError> {
    let value = match value {
        Some(value) => value,
        None => return Ok(default),
    };
    let parsed = value
        .parse()
        .map_err(|_| CronRuntimeConfigError::InvalidPositiveInteger {
            name,
            value: value.clone(),
        })?;
    if parsed == 0 {
        return Err(CronRuntimeConfigError::InvalidPositiveInteger { name, value });
    }
    Ok(parsed)
}

fn parse_enabled_jobs(
    raw: Option<String>,
    known_jobs: &[&str],
) -> Result<Vec<String>, CronRuntimeConfigError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for entry in raw.split(',') {
        let job = entry.trim();
        if job.is_empty() {
            return Err(CronRuntimeConfigError::EmptyEnabledJob);
        }
        if !known_jobs.contains(&job) {
            return Err(CronRuntimeConfigError::UnknownEnabledJob {
                name: job.to_owned(),
            });
        }
        if !seen.insert(job) {
            return Err(CronRuntimeConfigError::DuplicateEnabledJob {
                name: job.to_owned(),
            });
        }
        jobs.push(job.to_owned());
    }
    Ok(jobs)
}

#[derive(Debug, thiserror::Error)]
pub enum CronRuntimeConfigError {
    #[error("invalid {CRON_HEALTH_BIND_ADDR_ENV}: {value}")]
    InvalidHealthBindAddr {
        value: String,
        source: AddrParseError,
    },
    #[error("{name} must be a positive integer, got {value}")]
    InvalidPositiveInteger { name: &'static str, value: String },
    #[error("{CRON_ENABLED_JOBS_ENV} contains an empty job name")]
    EmptyEnabledJob,
    #[error("unknown enabled cron job: {name}")]
    UnknownEnabledJob { name: String },
    #[error("duplicate enabled cron job: {name}")]
    DuplicateEnabledJob { name: String },
    #[error("at least one cron job must be enabled outside local, test, and ephemeral stages")]
    NoEnabledJobs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn should_reject_unknown_enabled_job() {
        let values = HashMap::from([(CRON_ENABLED_JOBS_ENV, "unknown".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(
            result,
            Err(CronRuntimeConfigError::UnknownEnabledJob { .. })
        ));
    }

    #[test]
    fn should_reject_duplicate_enabled_job() {
        let values = HashMap::from([(CRON_ENABLED_JOBS_ENV, "known, known".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(
            result,
            Err(CronRuntimeConfigError::DuplicateEnabledJob { .. })
        ));
    }

    #[test]
    fn should_reject_empty_production_job_set() {
        let values = HashMap::from([(STAGE_ENV, "production".to_owned())]);
        let result = CronRuntimeConfig::from_getter(|name| values.get(name).cloned(), &["known"]);
        assert!(matches!(result, Err(CronRuntimeConfigError::NoEnabledJobs)));
    }
}
