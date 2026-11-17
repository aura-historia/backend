//! The PostgreSQL pool of a host process. PostgreSQL is reachable only on internal networks or a
//! Unix socket, so connections carry no TLS.

use crate::config::ConfigError;
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError};
use std::{env::VarError, str::FromStr};

pub const POSTGRES_HOST_ENV: &str = "POSTGRES_HOST";
pub const POSTGRES_PORT_ENV: &str = "POSTGRES_PORT";
pub const POSTGRES_DATABASE_ENV: &str = "POSTGRES_DATABASE";
pub const POSTGRES_USERNAME_ENV: &str = "POSTGRES_USERNAME";
pub const POSTGRES_PASSWORD_ENV: &str = "POSTGRES_PASSWORD";
pub const POSTGRES_MAX_CONNECTIONS_ENV: &str = "POSTGRES_MAX_CONNECTIONS";

#[derive(Debug, thiserror::Error)]
pub enum PostgresConfigError {
    #[error("invalid PostgreSQL configuration")]
    Config(#[from] ConfigError),
    #[error("invalid PostgreSQL pool configuration")]
    Pool(#[from] PostgresPoolConfigError),
}

/// Where the process connects. `POSTGRES_HOST` is a host name on the internal network or,
/// starting with `/`, the directory of PostgreSQL's Unix socket.
#[derive(Clone)]
pub struct PostgresTarget {
    host: String,
    port: u16,
    database: String,
}

impl PostgresTarget {
    pub fn from_env() -> Result<Self, PostgresConfigError> {
        Self::from_getter(&mut |name| std::env::var(name))
    }

    fn from_getter<F>(get: &mut F) -> Result<Self, PostgresConfigError>
    where
        F: FnMut(&str) -> Result<String, VarError>,
    {
        Ok(Self {
            host: required(get, POSTGRES_HOST_ENV)?,
            port: optional(get, POSTGRES_PORT_ENV, 5432)?,
            database: required(get, POSTGRES_DATABASE_ENV)?,
        })
    }

    /// One operator connection for migrations and role bootstrap.
    pub fn migration_config(
        &self,
        username: String,
        password: String,
    ) -> Result<PostgresPoolConfig, PostgresConfigError> {
        Ok(PostgresPoolConfig::migration(
            self.host.clone(),
            self.port,
            self.database.clone(),
            username,
            password,
        )?)
    }
}

/// The process pool from `POSTGRES_*`. Its size is fixed per process (`POSTGRES_MAX_CONNECTIONS`,
/// else `default_max_connections`), so the sum over all processes stays below the server's
/// `max_connections`.
pub fn pool_config_from_env(
    default_max_connections: u32,
) -> Result<PostgresPoolConfig, PostgresConfigError> {
    pool_config_from_getter(|name| std::env::var(name), default_max_connections)
}

pub fn pool_config_from_getter<F>(
    mut get: F,
    default_max_connections: u32,
) -> Result<PostgresPoolConfig, PostgresConfigError>
where
    F: FnMut(&str) -> Result<String, VarError>,
{
    let target = PostgresTarget::from_getter(&mut get)?;
    Ok(PostgresPoolConfig::service(
        target.host,
        target.port,
        target.database,
        required(&mut get, POSTGRES_USERNAME_ENV)?,
        required(&mut get, POSTGRES_PASSWORD_ENV)?,
        optional(
            &mut get,
            POSTGRES_MAX_CONNECTIONS_ENV,
            default_max_connections,
        )?,
    )?)
}

fn required<F>(get: &mut F, name: &str) -> Result<String, ConfigError>
where
    F: FnMut(&str) -> Result<String, VarError>,
{
    match get(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        Ok(_) | Err(VarError::NotPresent) => Err(ConfigError::Missing { name: name.into() }),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::Invalid { name: name.into() }),
    }
}

fn optional<F, T>(get: &mut F, name: &str, default: T) -> Result<T, ConfigError>
where
    F: FnMut(&str) -> Result<String, VarError>,
    T: FromStr,
{
    match get(name) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|_| ConfigError::Invalid { name: name.into() }),
        Err(VarError::NotPresent) => Ok(default),
        Err(VarError::NotUnicode(_)) => Err(ConfigError::Invalid { name: name.into() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, error::Error};

    fn values() -> HashMap<&'static str, String> {
        HashMap::from([
            (POSTGRES_HOST_ENV, "postgres".to_owned()),
            (POSTGRES_DATABASE_ENV, "aura".to_owned()),
            (POSTGRES_USERNAME_ENV, "aura_runtime".to_owned()),
            (POSTGRES_PASSWORD_ENV, "fixture-password".to_owned()),
        ])
    }

    fn from(
        values: &HashMap<&'static str, String>,
    ) -> Result<PostgresPoolConfig, PostgresConfigError> {
        pool_config_from_getter(
            |name| values.get(name).cloned().ok_or(VarError::NotPresent),
            10,
        )
    }

    #[test]
    fn should_build_a_tls_free_service_pool_with_the_process_default_size() {
        let config = from(&values()).unwrap();

        assert_eq!(5432, config.port());
        assert_eq!(10, config.max_connections());
        assert_eq!(None, config.tls());
        assert_eq!(
            platform_postgres::PostgresPoolTimeouts::service().acquire(),
            config.timeouts().unwrap().acquire()
        );
    }

    #[test]
    fn should_connect_through_a_unix_socket_directory() {
        let mut values = values();
        values.insert(POSTGRES_HOST_ENV, "/run/postgresql".to_owned());
        values.insert(POSTGRES_MAX_CONNECTIONS_ENV, "3".to_owned());

        let config = from(&values).unwrap();

        assert_eq!("/run/postgresql", config.host());
        assert_eq!(3, config.max_connections());
    }

    #[test]
    fn should_reject_invalid_configuration_without_echoing_the_password() {
        for (name, value) in [
            (POSTGRES_PORT_ENV, "not-a-port"),
            (POSTGRES_MAX_CONNECTIONS_ENV, "0"),
            (POSTGRES_PASSWORD_ENV, " "),
        ] {
            let mut values = values();
            values.insert(name, value.to_owned());
            let error = from(&values).unwrap_err();
            assert!(error.source().is_some(), "{name}");
            assert!(!format!("{error:?}").contains("fixture-password"));
        }
        assert!(matches!(
            from(&HashMap::new()),
            Err(PostgresConfigError::Config(ConfigError::Missing { name })) if name == POSTGRES_HOST_ENV
        ));
    }
}
