//! One-shot operator and scheduled commands of the host. Each command runs to completion, prints
//! a JSON report and exits non-zero on failure; schedules belong to the host's timers. The
//! [README](../README.md) owns the command contract.

mod cleanup;
mod command;
mod database;
mod fx_refresh;
mod periodic_match;
mod queue;
mod reprojection;

pub use command::{Command, OUTBOX, Selection};
pub use fx_refresh::{FxRefreshRun, capture_fx_rate_snapshot};

use application::error::BoxError;
use platform_process::postgres::pool_config_from_env;
use sqlx::PgPool;

/// Connections an operator command needs at most.
const POOL_SIZE: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("invalid configuration")]
    Config(#[source] BoxError),
    #[error("invalid arguments: {0}")]
    Usage(String),
    #[error("the {command} command failed")]
    Failed {
        command: &'static str,
        #[source]
        source: BoxError,
    },
}

impl OpsError {
    pub(crate) fn failed(command: &'static str, source: impl Into<BoxError>) -> Self {
        Self::Failed {
            command,
            source: source.into(),
        }
    }
}

/// Runs one command and returns its JSON report.
pub async fn run(command: Command) -> Result<serde_json::Value, OpsError> {
    match command {
        Command::Migrate { plan } => database::migrate(&plan).await,
        Command::InspectSchema => database::inspect_schema().await,
        Command::Bootstrap => database::bootstrap().await,
        Command::Inspect => queue::inspect(&pool().await?).await,
        Command::Redrive { scope, selection } => {
            queue::redrive(&pool().await?, &scope, &selection).await
        }
        Command::ReprojectAll {
            target,
            page_size,
            start_after,
        } => reprojection::reproject_all(&pool().await?, &target, page_size, start_after).await,
        Command::BackendCleanup => cleanup::run().await,
        Command::FxRefresh => fx_refresh::run().await,
        Command::PeriodicMatch => periodic_match::run().await,
    }
}

/// The runtime role's pool, from `POSTGRES_*`.
pub(crate) async fn pool() -> Result<PgPool, OpsError> {
    pool_config_from_env(POOL_SIZE)
        .map_err(|error| OpsError::Config(error.into()))?
        .connect()
        .await
        .map_err(|error| OpsError::Config(error.into()))
}
