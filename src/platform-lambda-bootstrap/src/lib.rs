use lambda_runtime::Context;
use platform_observability::{LogLevel, LoggingConfig};
use platform_postgres::PostgresPoolConfig;
use std::{
    env,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LambdaBootstrapConfigError {
    #[error("missing required Lambda configuration {name}")]
    Missing { name: &'static str },
    #[error("invalid Lambda configuration {name}")]
    Invalid { name: &'static str },
    #[error("invalid Lambda PostgreSQL configuration")]
    InvalidPostgres,
}

pub struct LambdaPostgresConfig {
    pool: PostgresPoolConfig,
}

impl LambdaPostgresConfig {
    pub fn from_env() -> Result<Self, LambdaBootstrapConfigError> {
        let host = required_env("POSTGRES_HOST")?;
        let database = required_env("POSTGRES_DATABASE")?;
        let username = required_env("POSTGRES_USERNAME")?;
        let password = required_env("POSTGRES_PASSWORD")?;
        let port = optional_env("POSTGRES_PORT", 5432)?;
        let max_connections = optional_env("POSTGRES_MAX_CONNECTIONS", 1)?;
        let root_certificate = PathBuf::from(required_env("POSTGRES_TLS_ROOT_CERT")?);
        let pool = PostgresPoolConfig::lambda(
            host,
            port,
            database,
            username,
            password,
            max_connections,
            root_certificate,
        )
        .map_err(|_| LambdaBootstrapConfigError::InvalidPostgres)?;

        Ok(Self { pool })
    }

    pub fn into_pool_config(self) -> PostgresPoolConfig {
        self.pool
    }
}

pub fn required_config_from_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    required_env(name)
}

pub fn logging_config_from_env() -> LoggingConfig {
    let level = env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}

pub fn log_cold_start(component: &'static str, initialization_started_at: Instant) {
    tracing::info!(
        event = "lambda.initialized",
        component,
        cold_start_duration_ms = initialization_started_at.elapsed().as_millis(),
    );
}

pub fn log_invocation_start(component: &'static str, context: &Context) {
    tracing::info!(
        event = "lambda.invocation.started",
        component,
        request_id = %context.request_id,
        remaining_budget_ms = remaining_budget_ms(context.deadline, epoch_millis()),
    );
}

fn remaining_budget_ms(deadline_epoch_ms: u64, now_epoch_ms: u64) -> u64 {
    deadline_epoch_ms.saturating_sub(now_epoch_ms)
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn required_env(name: &'static str) -> Result<String, LambdaBootstrapConfigError> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Err(LambdaBootstrapConfigError::Missing { name }),
        Err(env::VarError::NotUnicode(_)) => Err(LambdaBootstrapConfigError::Invalid { name }),
    }
}

fn optional_env<T>(name: &'static str, default: T) -> Result<T, LambdaBootstrapConfigError>
where
    T: std::str::FromStr,
{
    optional_env_from_result(name, env::var(name), default)
}

fn optional_env_from_result<T>(
    name: &'static str,
    value: Result<String, env::VarError>,
    default: T,
) -> Result<T, LambdaBootstrapConfigError>
where
    T: std::str::FromStr,
{
    match value {
        Ok(value) => value
            .parse()
            .map_err(|_| LambdaBootstrapConfigError::Invalid { name }),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(env::VarError::NotUnicode(_)) => Err(LambdaBootstrapConfigError::Invalid { name }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_default_when_optional_configuration_is_absent() {
        let value = optional_env_from_result("POSTGRES_PORT", Err(env::VarError::NotPresent), 5432);

        assert_eq!(Ok(5432), value);
    }

    #[test]
    fn should_reject_invalid_optional_configuration_without_echoing_its_value() {
        let value = optional_env_from_result("POSTGRES_PORT", Ok(String::from("not-a-port")), 5432);

        assert_eq!(
            Err(LambdaBootstrapConfigError::Invalid {
                name: "POSTGRES_PORT"
            }),
            value
        );
    }

    #[test]
    fn should_clamp_elapsed_invocation_deadline_to_zero() {
        assert_eq!(0, remaining_budget_ms(100, 101));
    }
}
