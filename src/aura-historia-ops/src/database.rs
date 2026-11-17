//! Schema migration, read-only schema inspection and first-time role bootstrap. Each command
//! connects as its own role over one operator connection; none of them reads another role's
//! credentials.

mod bootstrap;
mod operations;

use crate::OpsError;
use platform_process::{config, postgres::PostgresTarget};
use serde_json::Value;
use std::path::Path;

pub(crate) const MIGRATOR_USERNAME_ENV: &str = "POSTGRES_MIGRATOR_USERNAME";
pub(crate) const MIGRATOR_PASSWORD_ENV: &str = "POSTGRES_MIGRATOR_PASSWORD";
const INSPECTOR_USERNAME_ENV: &str = "POSTGRES_INSPECTOR_USERNAME";
const INSPECTOR_PASSWORD_ENV: &str = "POSTGRES_INSPECTOR_PASSWORD";

static ROOT_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Applies the reviewed migration plan in `plan` as the migrator role.
pub(crate) async fn migrate(plan: &Path) -> Result<Value, OpsError> {
    let request =
        operations::read_plan(plan).map_err(|error| OpsError::failed("migrate", error))?;
    let config =
        operations::OperationConfig::from_env(MIGRATOR_USERNAME_ENV, MIGRATOR_PASSWORD_ENV)?;
    let result = operations::migrate(&config, request)
        .await
        .map_err(|error| OpsError::failed("migrate", error))?;
    serde_json::to_value(result).map_err(|error| OpsError::failed("migrate", error))
}

/// The migration ledger, its fingerprint and the schema catalogue, read as the inspector role.
pub(crate) async fn inspect_schema() -> Result<Value, OpsError> {
    let config =
        operations::OperationConfig::from_env(INSPECTOR_USERNAME_ENV, INSPECTOR_PASSWORD_ENV)?;
    let result = operations::inspect(&config)
        .await
        .map_err(|error| OpsError::failed("inspect-schema", error))?;
    serde_json::to_value(result).map_err(|error| OpsError::failed("inspect-schema", error))
}

/// Creates the application roles as an administrator and applies every migration as the
/// migrator.
pub(crate) async fn bootstrap() -> Result<Value, OpsError> {
    let config = bootstrap::BootstrapConfig::from_env()?;
    let outcome = bootstrap::initialize_database(&config)
        .await
        .map_err(|error| OpsError::failed("bootstrap", error))?;
    Ok(serde_json::json!({
        "status": "BOOTSTRAPPED",
        "replication_role_kept": outcome.replication_role_kept,
    }))
}

fn target() -> Result<PostgresTarget, OpsError> {
    PostgresTarget::from_env().map_err(|error| OpsError::Config(error.into()))
}

fn required(name: &str) -> Result<String, OpsError> {
    config::required(name).map_err(|error| OpsError::Config(error.into()))
}
