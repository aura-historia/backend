//! Periodic saved-search matching: one bounded run, started by the host's `ops periodic-match`
//! command. The caller owns process setup (logging, panic hook, signal registration).

mod execution;
mod wiring;

pub use execution::ExecutionError;
use execution::STARTUP_LIMIT;
use std::{future::Future, time::Instant};

const JOB: &str = "search-filter-periodic-match";

/// Runs one matching pass until it completes, fails, times out or `shutdown` resolves, and logs
/// its start and terminal outcome with the stage and source revision.
pub async fn run(shutdown: impl Future<Output = ()>) -> Result<(), ExecutionError> {
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
    tracing::info!(job = JOB, stage, revision,
        started_at = %time::OffsetDateTime::now_utc(), "cron.matching.started");
    let result = execution::run(wiring::build_from_env(), shutdown, STARTUP_LIMIT).await;
    let outcome = result
        .as_ref()
        .err()
        .map_or("success", ExecutionError::outcome);
    macro_rules! log_terminal {
        ($log:ident) => {
            tracing::$log!(job = JOB, stage, revision, outcome,
                finished_at = %time::OffsetDateTime::now_utc(), duration_ms = started.elapsed().as_millis(),
                "cron.matching.terminal")
        };
    }
    // The terminal outcome of a run that did not succeed is a failure to report, whatever level
    // the job logs at; only a successful run is routine.
    match &result {
        Ok(()) => log_terminal!(info),
        Err(_) => log_terminal!(warn),
    }
    result
}
