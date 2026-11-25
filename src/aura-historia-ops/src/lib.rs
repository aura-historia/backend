//! One-shot operator and scheduled commands of the host. Each command runs to completion, prints
//! a JSON report and exits non-zero on failure; schedules belong to the host's timers. The
//! [README](../README.md) owns the command contract.

mod cleanup;
mod command;
mod database;
mod fx_refresh;
mod periodic_match;
mod queue;
mod renormalization;
mod reprojection;

pub use command::{Command, OUTBOX, Selection};
pub use fx_refresh::{FxRefreshRun, capture_fx_rate_snapshot};
pub use renormalization::RenormalizationOptions;

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
    /// What an operator may read about the failure: a schema operation's static outcome or the
    /// setting at fault. Other causes can carry configuration or provider text and stay unnamed.
    pub fn reason(&self) -> Option<String> {
        database::operation_outcome(self)
            .or_else(|| platform_process::config::find_config_error(self).map(ToString::to_string))
    }

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
        Command::RenormalizeRaw(options) => {
            renormalization::renormalize_raw(&pool().await?, &options).await
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use platform_process::config::ConfigError;

    #[test]
    fn should_name_only_the_setting_of_a_configuration_failure() {
        let error = OpsError::Config(Box::new(ConfigError::Missing {
            name: "FXRATES_API_KEY".to_owned(),
        }));

        assert_eq!(
            Some("missing required configuration FXRATES_API_KEY".to_owned()),
            error.reason()
        );
    }

    #[test]
    fn should_leave_other_failure_causes_unnamed() {
        let error = OpsError::failed(
            "fx-refresh",
            std::io::Error::other("provider said: key abc"),
        );

        assert_eq!(None, error.reason());
    }
}
