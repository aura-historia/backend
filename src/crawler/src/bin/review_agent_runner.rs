//! `review-agent-runner`: the review agents of `agent` authoring mode, working the review console's
//! pending reviews in passes, each review by a fresh author and then a fresh reviewer `claude -p`
//! process of the Claude Code CLI that is logged in on the machine.
//!
//! ```text
//! review-agent-runner          # the service: a pass every interval until stopped, the daily digest when due
//! review-agent-runner --once   # a manual run: one pass, then a digest of the last day
//! ```
//!
//! Its settings are environment variables, listed with their defaults in
//! `docs/crawler/operations.md#review-agent-runner`; it exits with 2 on a setting it does not
//! accept. The service logs a failed pass and tries again after the interval; `--once` exits with
//! 1 when it refuses to run or its pass fails. SIGTERM or SIGINT stops it: it starts no further
//! agent process, stops the running ones as at their timeout, SIGINT first, and records their
//! cost, then exits with 143 or 130 within [`STOP_DEADLINE`]; what still runs then is killed, and
//! a second signal does not shorten the stop. The cooldowns and pauses it set are already written.
//! The JSON log goes to stdout.

use crawler::review_agent_runner::{
    RunMode, RunnerError, RunnerSettings, RunnerStop, STOP_DEADLINE, current_location, run,
    run_every_interval,
};
use std::process::ExitCode;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(level))
        .init();

    let settings = match current_location().and_then(|location| {
        RunnerSettings::from_args_and_env(
            std::env::args().skip(1),
            |name| std::env::var(name).ok(),
            &location,
        )
    }) {
        Ok(settings) => settings,
        Err(settings_error) => {
            error!(error = %settings_error, "Invalid review agent runner settings");
            return ExitCode::from(2);
        }
    };
    let stop = RunnerStop::default();
    let work = async {
        match settings.mode {
            RunMode::Once => run(&settings, &stop).await.map(drop),
            RunMode::Loop => {
                run_every_interval(&settings, &stop).await;
                Ok(())
            }
        }
    };
    tokio::pin!(work);
    let (signal, exit_code) = tokio::select! {
        // The signal handlers are installed when `stop_signal` is first polled: before the run
        // starts any child, so a stop signal never ends the runner with its default action.
        biased;
        stopped_by = stop_signal() => stopped_by,
        outcome = &mut work => return match outcome {
            Ok(()) => ExitCode::SUCCESS,
            Err(run_error) => {
                log_failed_run(&run_error);
                ExitCode::FAILURE
            }
        },
    };
    stop_work(&stop, work, signal).await;
    warn!(signal, "Review agent runner stopped by a signal");
    ExitCode::from(exit_code)
}

/// Requests `stop` on `signal` and waits for `work` to stop its agent processes and record them,
/// for at most [`STOP_DEADLINE`]. Work still running then is dropped as the runner exits, which
/// kills every agent process group left.
async fn stop_work(
    stop: &RunnerStop,
    work: impl Future<Output = Result<(), RunnerError>>,
    signal: &str,
) {
    info!(
        signal,
        stop_deadline_seconds = STOP_DEADLINE.as_secs(),
        "Review agent runner stopping: no agent process starts, and the running ones are stopped and recorded"
    );
    stop.request();
    match tokio::time::timeout(STOP_DEADLINE, work).await {
        Ok(Ok(())) => {}
        Ok(Err(run_error)) => log_failed_run(&run_error),
        Err(_) => warn!(
            stop_deadline_seconds = STOP_DEADLINE.as_secs(),
            "Review agent runner's stop outlasted its deadline; what still runs is killed"
        ),
    }
}

fn log_failed_run(run_error: &RunnerError) {
    error!(
        error = %run_error,
        cause = ?std::error::Error::source(run_error),
        "Review agent runner stopped"
    );
}

/// The first stop signal and the exit code it ends the runner with. Without the handlers it
/// never comes, and the signals keep their default action. Once installed, the handlers stay for
/// the process: a later signal, such as `dev`'s SIGTERM after the Ctrl+C that reached its whole
/// terminal, is ignored and does not shorten the stop.
async fn stop_signal() -> (&'static str, u8) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut terminate), Ok(mut interrupt)) => tokio::select! {
                _ = terminate.recv() => ("SIGTERM", 143),
                _ = interrupt.recv() => ("SIGINT", 130),
            },
            _ => std::future::pending().await,
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => ("Ctrl+C", 130),
            Err(_) => std::future::pending().await,
        }
    }
}
