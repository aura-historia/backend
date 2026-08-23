use aura_historia_cron::scheduled_job::CronJobExecutionError;
use aura_historia_cron::wiring::{WiringError, build_from_env};
use aura_historia_cron::{
    CRON_ENABLED_JOBS_ENV, CronRuntimeConfig, JobRegistration, run_until_shutdown,
};
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
    let run_once = parse_run_once()?;
    let config = CronRuntimeConfig::from_env(&[SEARCH_FILTER_PERIODIC_MATCH_JOB])?;
    let (job, schedule, max_run_duration) = build_from_env().await?;
    if run_once {
        return job.execute().await.map_err(MainError::Job);
    }
    if !config
        .enabled_jobs()
        .iter()
        .any(|name| name == SEARCH_FILTER_PERIODIC_MATCH_JOB)
    {
        return Err(MainError::NoJobsWired {
            env: CRON_ENABLED_JOBS_ENV,
        });
    }
    run_until_shutdown(
        config,
        vec![JobRegistration {
            name: SEARCH_FILTER_PERIODIC_MATCH_JOB,
            schedule,
            max_run_duration: Some(max_run_duration),
            job,
        }],
        shutdown_signal(),
    )
    .await?;
    Ok(())
}
fn parse_run_once() -> Result<bool, MainError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(false);
    }
    if args == ["--run-once", SEARCH_FILTER_PERIODIC_MATCH_JOB] {
        return Ok(true);
    }
    Err(MainError::InvalidArguments)
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "cron.shutdown.sigterm_setup_failed");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error(transparent)]
    Config(#[from] aura_historia_cron::CronRuntimeConfigError),
    #[error(transparent)]
    Wiring(#[from] WiringError),
    #[error(transparent)]
    Runtime(#[from] aura_historia_cron::CronRuntimeError),
    #[error("no cron jobs are wired; configure {env}")]
    NoJobsWired { env: &'static str },
    #[error("usage: aura-historia-cron [--run-once search-filter-periodic-match]")]
    InvalidArguments,
    #[error("cron job failed")]
    Job(#[source] CronJobExecutionError),
}
