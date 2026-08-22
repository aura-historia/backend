use aura_historia_cron::{CRON_ENABLED_JOBS_ENV, CronRuntimeConfig};
use platform_observability::{LogLevel, LoggingConfig, init};

const SEARCH_FILTER_PERIODIC_MATCH_JOB: &str = "search-filter-periodic-match";

#[tokio::main]
async fn main() -> Result<(), MainError> {
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));
    let _config = CronRuntimeConfig::from_env(&[SEARCH_FILTER_PERIODIC_MATCH_JOB])?;
    Err(MainError::NoJobsWired {
        env: CRON_ENABLED_JOBS_ENV,
    })
}

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error(transparent)]
    Config(#[from] aura_historia_cron::CronRuntimeConfigError),
    #[error("no cron jobs are wired yet; configure {env} only after the matcher wiring iteration")]
    NoJobsWired { env: &'static str },
}
