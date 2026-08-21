use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::CaptureFxRateSnapshotHandler;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresPoolConfig, SqlxUnitOfWork};
use serde_json::Value;
use std::{fmt::Display, str::FromStr};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init(logging_config_from_env());

    let pool = postgres_config_from_env()?.connect().await?;
    let token = std::env::var("FXRATES_API_TOKEN")
        .map_err(|_| Error::from("missing required environment variable FXRATES_API_TOKEN"))?;
    let snapshots = CaptureFxRateSnapshotHandler::new(
        FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    );

    debug!("FX rate Lambda initialized");
    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async { handler(event, &snapshots).await },
    ))
    .await
}

fn logging_config_from_env() -> LoggingConfig {
    let level = std::env::var("LOG_LEVEL")
        .ok()
        .as_deref()
        .and_then(LogLevel::parse)
        .unwrap_or_default();
    LoggingConfig::new(level)
}

fn postgres_config_from_env() -> Result<PostgresPoolConfig, Error> {
    let host = required_env("POSTGRES_HOST")?;
    let database = required_env("POSTGRES_DATABASE")?;
    let username = required_env("POSTGRES_USERNAME")?;
    let password = required_env("POSTGRES_PASSWORD")?;
    let port = optional_env("POSTGRES_PORT", 5432)?;
    let max_connections = optional_env("POSTGRES_MAX_CONNECTIONS", 2)?;

    PostgresPoolConfig::new(host, port, database, username, password, max_connections)
        .map_err(|error| config_error(error.to_string()))
}

fn required_env(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|error| config_error(format!("failed to read {name}: {error}")))
}

fn optional_env<T>(name: &str, default: T) -> Result<T, Error>
where
    T: FromStr,
    T::Err: Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| config_error(format!("invalid {name} value: {error}"))),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(config_error(format!("failed to read {name}: {error}"))),
    }
}

fn config_error(message: String) -> Error {
    std::io::Error::other(message).into()
}
