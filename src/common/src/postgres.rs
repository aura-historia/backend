use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::fmt;
use std::num::ParseIntError;
use std::time::Duration;

pub const POSTGRES_HOST_ENV: &str = "POSTGRES_HOST";
pub const POSTGRES_PORT_ENV: &str = "POSTGRES_PORT";
pub const POSTGRES_DATABASE_ENV: &str = "POSTGRES_DATABASE";
pub const POSTGRES_USERNAME_ENV: &str = "POSTGRES_USERNAME";
pub const POSTGRES_PASSWORD_ENV: &str = "POSTGRES_PASSWORD";
pub const POSTGRES_MAX_CONNECTIONS_ENV: &str = "POSTGRES_MAX_CONNECTIONS";

const DEFAULT_POSTGRES_PORT: u16 = 5432;
const DEFAULT_POSTGRES_MAX_CONNECTIONS: u32 = 2;
const DEFAULT_ACQUIRE_TIMEOUT_SECONDS: u64 = 5;

#[derive(Clone, PartialEq, Eq)]
pub struct PostgresPoolConfig {
    host: String,
    port: u16,
    database: String,
    username: String,
    password: String,
    max_connections: u32,
}

impl fmt::Debug for PostgresPoolConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresPoolConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

impl PostgresPoolConfig {
    pub fn from_env() -> Result<Self, PostgresConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, PostgresConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let host = required_env(&mut get, POSTGRES_HOST_ENV)?;
        let database = required_env(&mut get, POSTGRES_DATABASE_ENV)?;
        let username = required_env(&mut get, POSTGRES_USERNAME_ENV)?;
        let password = required_env(&mut get, POSTGRES_PASSWORD_ENV)?;
        let port = parse_optional_env(&mut get, POSTGRES_PORT_ENV, DEFAULT_POSTGRES_PORT)?;
        let max_connections = parse_optional_env(
            &mut get,
            POSTGRES_MAX_CONNECTIONS_ENV,
            DEFAULT_POSTGRES_MAX_CONNECTIONS,
        )?;

        if max_connections == 0 {
            return Err(PostgresConfigError::ZeroMaxConnections);
        }

        Ok(Self {
            host,
            port,
            database,
            username,
            password,
            max_connections,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn max_connections(&self) -> u32 {
        self.max_connections
    }

    pub fn connect_options(&self) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.database)
            .username(&self.username)
            .password(&self.password)
    }

    pub fn pool_options(&self) -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(self.max_connections)
            .acquire_timeout(Duration::from_secs(DEFAULT_ACQUIRE_TIMEOUT_SECONDS))
    }

    pub async fn connect(&self) -> Result<PgPool, sqlx::Error> {
        self.pool_options()
            .connect_with(self.connect_options())
            .await
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PostgresConfigError {
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("invalid integer in environment variable {name}: {value}")]
    InvalidInteger {
        name: &'static str,
        value: String,
        source: ParseIntError,
    },
    #[error("POSTGRES_MAX_CONNECTIONS must be greater than zero")]
    ZeroMaxConnections,
}

pub async fn connect_from_env() -> Result<PgPool, PostgresConnectError> {
    PostgresPoolConfig::from_env()?
        .connect()
        .await
        .map_err(PostgresConnectError::Connect)
}

#[derive(thiserror::Error, Debug)]
pub enum PostgresConnectError {
    #[error(transparent)]
    Config(#[from] PostgresConfigError),
    #[error("failed to connect to Postgres")]
    Connect(#[source] sqlx::Error),
}

fn required_env<F>(get: &mut F, name: &'static str) -> Result<String, PostgresConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    get(name).ok_or(PostgresConfigError::MissingEnv { name })
}

fn parse_optional_env<F, T>(
    get: &mut F,
    name: &'static str,
    default: T,
) -> Result<T, PostgresConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
    T: std::str::FromStr<Err = ParseIntError>,
{
    match get(name) {
        Some(value) => value
            .parse::<T>()
            .map_err(|source| PostgresConfigError::InvalidInteger {
                name,
                value,
                source,
            }),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn should_build_config_when_required_values_exist() {
        let values = env_values([
            (POSTGRES_HOST_ENV, "localhost"),
            (POSTGRES_DATABASE_ENV, "aura"),
            (POSTGRES_USERNAME_ENV, "postgres"),
            (POSTGRES_PASSWORD_ENV, "secret"),
        ]);

        let config = PostgresPoolConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(config, Ok(ref value)
            if value.host() == "localhost"
                && value.port() == 5432
                && value.database() == "aura"
                && value.username() == "postgres"
                && value.max_connections() == 2));
    }

    #[test]
    fn should_override_optional_values_when_env_exists() {
        let values = env_values([
            (POSTGRES_HOST_ENV, "db.local"),
            (POSTGRES_PORT_ENV, "15432"),
            (POSTGRES_DATABASE_ENV, "backend"),
            (POSTGRES_USERNAME_ENV, "app"),
            (POSTGRES_PASSWORD_ENV, "secret"),
            (POSTGRES_MAX_CONNECTIONS_ENV, "4"),
        ]);

        let config = PostgresPoolConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(config, Ok(ref value)
            if value.host() == "db.local"
                && value.port() == 15432
                && value.database() == "backend"
                && value.username() == "app"
                && value.max_connections() == 4));
    }

    #[test]
    fn should_fail_when_port_is_not_an_integer() {
        let values = env_values([
            (POSTGRES_HOST_ENV, "db.local"),
            (POSTGRES_PORT_ENV, "not-a-number"),
            (POSTGRES_DATABASE_ENV, "backend"),
            (POSTGRES_USERNAME_ENV, "app"),
            (POSTGRES_PASSWORD_ENV, "secret"),
        ]);

        let config = PostgresPoolConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(PostgresConfigError::InvalidInteger { name, .. }) if name == POSTGRES_PORT_ENV
        ));
    }

    #[test]
    fn should_redact_password_in_debug_output() {
        let values = env_values([
            (POSTGRES_HOST_ENV, "localhost"),
            (POSTGRES_DATABASE_ENV, "aura"),
            (POSTGRES_USERNAME_ENV, "postgres"),
            (POSTGRES_PASSWORD_ENV, "very-secret"),
        ]);

        let output = match PostgresPoolConfig::from_getter(|name| values.get(name).cloned()) {
            Ok(config) => format!("{config:?}"),
            Err(error) => format!("unexpected error: {error}"),
        };

        assert!(!output.contains("very-secret"));
        assert!(output.contains("<redacted>"));
    }

    #[test]
    fn should_fail_when_required_value_is_missing() {
        let values = env_values([
            (POSTGRES_DATABASE_ENV, "aura"),
            (POSTGRES_USERNAME_ENV, "postgres"),
            (POSTGRES_PASSWORD_ENV, "secret"),
        ]);

        let config = PostgresPoolConfig::from_getter(|name| values.get(name).cloned());

        assert!(
            matches!(config, Err(PostgresConfigError::MissingEnv { name }) if name == POSTGRES_HOST_ENV)
        );
    }

    #[test]
    fn should_fail_when_max_connections_is_zero() {
        let values = env_values([
            (POSTGRES_HOST_ENV, "localhost"),
            (POSTGRES_DATABASE_ENV, "aura"),
            (POSTGRES_USERNAME_ENV, "postgres"),
            (POSTGRES_PASSWORD_ENV, "secret"),
            (POSTGRES_MAX_CONNECTIONS_ENV, "0"),
        ]);

        let config = PostgresPoolConfig::from_getter(|name| values.get(name).cloned());

        assert!(matches!(
            config,
            Err(PostgresConfigError::ZeroMaxConnections)
        ));
    }

    fn env_values<const N: usize>(
        values: [(&'static str, &'static str); N],
    ) -> HashMap<&'static str, String> {
        values
            .into_iter()
            .map(|(key, value)| (key, value.to_owned()))
            .collect()
    }
}
