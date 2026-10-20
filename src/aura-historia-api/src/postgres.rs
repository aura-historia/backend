use crate::ApiStateError;
use platform_postgres::{PostgresConnectError, PostgresPoolConfig};
use sqlx::PgPool;
use std::path::PathBuf;

const POSTGRES_HOST_ENV: &str = "POSTGRES_HOST";
const POSTGRES_PORT_ENV: &str = "POSTGRES_PORT";
const POSTGRES_DATABASE_ENV: &str = "POSTGRES_DATABASE";
const POSTGRES_USERNAME_ENV: &str = "POSTGRES_USERNAME";
const POSTGRES_PASSWORD_ENV: &str = "POSTGRES_PASSWORD";
const POSTGRES_MAX_CONNECTIONS_ENV: &str = "POSTGRES_MAX_CONNECTIONS";
const POSTGRES_TLS_ROOT_CERT_ENV: &str = "POSTGRES_TLS_ROOT_CERT";
const DEFAULT_POSTGRES_PORT: u16 = 5432;
const DEFAULT_POSTGRES_MAX_CONNECTIONS: u32 = 1;

pub(super) async fn pool_from_env() -> Result<PgPool, ApiStateError> {
    let host = required_postgres_env(POSTGRES_HOST_ENV)?;
    let database = required_postgres_env(POSTGRES_DATABASE_ENV)?;
    let username = required_postgres_env(POSTGRES_USERNAME_ENV)?;
    let password = required_postgres_env(POSTGRES_PASSWORD_ENV)?;
    let port = optional_postgres_env(POSTGRES_PORT_ENV, DEFAULT_POSTGRES_PORT)?;
    let max_connections = optional_postgres_env(
        POSTGRES_MAX_CONNECTIONS_ENV,
        DEFAULT_POSTGRES_MAX_CONNECTIONS,
    )?;
    let root_certificate = PathBuf::from(required_postgres_env(POSTGRES_TLS_ROOT_CERT_ENV)?);
    let config = PostgresPoolConfig::lambda(
        host,
        port,
        database,
        username,
        password,
        max_connections,
        root_certificate,
    )
    .map_err(|error| match error {
        platform_postgres::PostgresPoolConfigError::ZeroMaxConnections
        | platform_postgres::PostgresPoolConfigError::MigrationMaxConnectionsMustBeOne => {
            ApiStateError::InvalidPostgresMaxConnections
        }
        platform_postgres::PostgresPoolConfigError::EmptyRootCertificate => {
            ApiStateError::InvalidPostgresTlsRootCertificate
        }
    })?;

    Ok(config
        .connect()
        .await
        .map_err(|_| PostgresConnectError::Connect)?)
}

fn required_postgres_env(name: &'static str) -> Result<String, ApiStateError> {
    std::env::var(name).map_err(|_| ApiStateError::MissingEnv { name })
}

fn optional_postgres_env<T>(name: &'static str, default: T) -> Result<T, ApiStateError>
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| ApiStateError::InvalidPostgresInteger { name, value }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(ApiStateError::MissingEnv { name }),
    }
}
