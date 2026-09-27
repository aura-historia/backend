mod execution;
mod google_adc;
mod wiring;

use execution::{ExecutionError, STARTUP_LIMIT};
use platform_observability::{LogLevel, LoggingConfig, init};
use std::time::Instant;
use tokio::signal::unix::{SignalKind, signal};

fn accept_arguments(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(), &'static str> {
    if args.into_iter().next().is_some() {
        Err("aura-historia-cron accepts no arguments")
    } else {
        Ok(())
    }
}

fn main() -> Result<(), &'static str> {
    accept_arguments(std::env::args_os().skip(1))?;
    // Tokio's default panic hook includes the payload; do not print provider or credential data.
    std::panic::set_hook(Box::new(|_| eprintln!("periodic matching task panicked")));
    google_adc::materialize_google_application_credentials_from_env()
        .map_err(|_| "failed to prepare Google application credentials")?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| "failed to start async runtime")?;
    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), &'static str> {
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));
    // Register both signal handlers before async provider startup. A setup failure is fatal.
    let mut term = signal(SignalKind::terminate()).map_err(|_| "failed to register SIGTERM")?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|_| "failed to register SIGINT")?;
    let shutdown = async move {
        tokio::select! {
            _ = term.recv() => {},
            _ = interrupt.recv() => {},
        }
    };
    let stage = std::env::var("STAGE").unwrap_or_default();
    let stage = match stage.as_str() {
        "dev" => "dev",
        "prod" => "prod",
        "local" => "local",
        "test" => "test",
        "ephemeral" => "ephemeral",
        _ => "unknown",
    };
    let revision = std::env::var("AURA_HISTORIA_SOURCE_REVISION").unwrap_or_default();
    let revision = if revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        revision.as_str()
    } else {
        "unknown"
    };
    let started = Instant::now();
    tracing::info!(job = "search-filter-periodic-match", stage, revision,
        started_at = %time::OffsetDateTime::now_utc(), "cron.matching.started");
    let result = execution::run(wiring::build_from_env(), shutdown, STARTUP_LIMIT).await;
    let outcome = match &result {
        Ok(()) => "success",
        Err(ExecutionError::StartupFailed) => "startup_failed",
        Err(ExecutionError::StartupTimedOut) => "startup_timed_out",
        Err(ExecutionError::Cancelled) => "cancelled",
        Err(ExecutionError::MatchingTimedOut) => "matching_timed_out",
        Err(ExecutionError::FailedFilters(_)) => "incomplete",
        Err(ExecutionError::ServiceFailed) => "service_failed",
        Err(ExecutionError::Panicked) => "panicked",
        Err(ExecutionError::TaskFailed) => "task_failed",
    };
    tracing::info!(job = "search-filter-periodic-match", stage, revision, outcome,
        finished_at = %time::OffsetDateTime::now_utc(), duration_ms = started.elapsed().as_millis(),
        "cron.matching.terminal");
    result.map_err(|_| "periodic matching did not complete successfully")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_any_arguments_without_accessing_providers() {
        assert!(accept_arguments(std::iter::empty()).is_ok());
        assert!(accept_arguments(["--run-once".into()]).is_err());
        assert!(accept_arguments(["search-filter-periodic-match".into()]).is_err());
    }
}
