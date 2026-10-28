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
//! 1 when it refuses to run or its pass fails. SIGTERM or SIGINT stops it at once with its agent
//! processes (exit code 143 or 130); the cooldowns and pauses it set are already written. The JSON
//! log goes to stdout.

use crawler::review_agent_runner::{
    RunMode, RunnerSettings, current_location, run, run_every_interval,
};
use std::process::ExitCode;
use tracing::{error, warn};
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
    let work = async {
        match settings.mode {
            RunMode::Once => run(&settings).await.map(drop),
            RunMode::Loop => {
                run_every_interval(&settings).await;
                Ok(())
            }
        }
    };
    tokio::select! {
        outcome = work => match outcome {
            Ok(_) => ExitCode::SUCCESS,
            Err(run_error) => {
                error!(
                    error = %run_error,
                    cause = ?std::error::Error::source(&run_error),
                    "Review agent runner stopped"
                );
                ExitCode::FAILURE
            }
        },
        (signal, exit_code) = stop_signal() => {
            // Leaving the select drops the run: each agent process it waits for is killed, and
            // its `review-mcp` ends with the CLI's stdio.
            warn!(signal, "Review agent runner stopped by a signal, with its agent processes");
            ExitCode::from(exit_code)
        }
    }
}

/// The first stop signal and the exit code it ends the runner with. Without the handlers it
/// never comes, and the signals keep their default action.
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
