use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError};
use std::{num::ParseIntError, path::PathBuf};

pub(super) fn pool_config_from_env() -> Result<PostgresPoolConfig, PostgresConfigError> {
    pool_config_from_getter(std::env::var)
}

fn pool_config_from_getter<F>(mut get: F) -> Result<PostgresPoolConfig, PostgresConfigError>
where
    F: FnMut(&'static str) -> Result<String, std::env::VarError>,
{
    let host = required(&mut get, "POSTGRES_HOST")?;
    let database = required(&mut get, "POSTGRES_DATABASE")?;
    let username = required(&mut get, "POSTGRES_USERNAME")?;
    let password = required(&mut get, "POSTGRES_PASSWORD")?;
    let port = optional_integer(&mut get, "POSTGRES_PORT", 5432)?;
    let max_connections = optional_integer(&mut get, "POSTGRES_MAX_CONNECTIONS", 1)?;
    let root_certificate = PathBuf::from(required(&mut get, "POSTGRES_TLS_ROOT_CERT")?);
    Ok(PostgresPoolConfig::lambda(
        host,
        port,
        database,
        username,
        password,
        max_connections,
        root_certificate,
    )?)
}

fn required<F>(get: &mut F, name: &'static str) -> Result<String, PostgresConfigError>
where
    F: FnMut(&'static str) -> Result<String, std::env::VarError>,
{
    get(name).map_err(|error| match error {
        std::env::VarError::NotPresent => PostgresConfigError::Missing { name },
        std::env::VarError::NotUnicode(_) => PostgresConfigError::InvalidEnvironment { name },
    })
}

fn optional_integer<F, T>(
    get: &mut F,
    name: &'static str,
    default: T,
) -> Result<T, PostgresConfigError>
where
    F: FnMut(&'static str) -> Result<String, std::env::VarError>,
    T: std::str::FromStr<Err = ParseIntError>,
{
    match get(name) {
        Ok(value) => value
            .parse()
            .map_err(|source| PostgresConfigError::Integer { name, source }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(PostgresConfigError::InvalidEnvironment { name })
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PostgresConfigError {
    #[error("missing required environment variable {name}")]
    Missing { name: &'static str },
    #[error("invalid environment variable {name}")]
    InvalidEnvironment { name: &'static str },
    #[error("invalid integer in environment variable {name}")]
    Integer {
        name: &'static str,
        #[source]
        source: ParseIntError,
    },
    #[error("invalid PostgreSQL pool configuration")]
    Pool(#[from] PostgresPoolConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, error::Error};

    fn values() -> HashMap<&'static str, String> {
        HashMap::from([
            ("POSTGRES_HOST", "localhost".to_owned()),
            ("POSTGRES_DATABASE", "fixture".to_owned()),
            ("POSTGRES_USERNAME", "fixture".to_owned()),
            ("POSTGRES_PASSWORD", "fixture-password".to_owned()),
            ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem".to_owned()),
        ])
    }

    #[test]
    fn should_preserve_default_pool_profile_with_only_postgres_configuration()
    -> Result<(), Box<dyn Error>> {
        let config = pool_config_from_getter(|name| {
            values()
                .get(name)
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        })?;
        assert_eq!(5432, config.port());
        assert_eq!(1, config.max_connections());
        assert_eq!(0, config.min_connections());
        assert_eq!(
            Some(platform_postgres::PostgresPoolTimeouts::lambda()),
            config.timeouts()
        );
        assert!(config.tls().is_some());
        Ok(())
    }

    #[test]
    fn should_reject_invalid_pool_configuration_and_preserve_causes() {
        for (name, value) in [
            ("POSTGRES_PORT", "not-a-port"),
            ("POSTGRES_MAX_CONNECTIONS", "0"),
            ("POSTGRES_TLS_ROOT_CERT", ""),
        ] {
            let mut config = values();
            config.insert(name, value.to_owned());
            let error = pool_config_from_getter(|name| {
                config
                    .get(name)
                    .cloned()
                    .ok_or(std::env::VarError::NotPresent)
            })
            .unwrap_err();
            assert!(error.source().is_some());
            assert!(!format!("{error:?}").contains("fixture-password"));
        }
        let error = pool_config_from_getter(|_| Err(std::env::VarError::NotPresent)).unwrap_err();
        assert!(matches!(
            error,
            PostgresConfigError::Missing {
                name: "POSTGRES_HOST"
            }
        ));
    }

    #[test]
    fn should_reject_non_unicode_optional_configuration_instead_of_using_defaults() {
        use std::os::unix::ffi::OsStringExt;
        for invalid_name in ["POSTGRES_PORT", "POSTGRES_MAX_CONNECTIONS"] {
            let error = pool_config_from_getter(|name| {
                if name == invalid_name {
                    Err(std::env::VarError::NotUnicode(
                        std::ffi::OsString::from_vec(vec![0xff]),
                    ))
                } else {
                    values()
                        .get(name)
                        .cloned()
                        .ok_or(std::env::VarError::NotPresent)
                }
            })
            .unwrap_err();
            assert!(
                matches!(error, PostgresConfigError::InvalidEnvironment { name } if name == invalid_name)
            );
        }
    }
}
