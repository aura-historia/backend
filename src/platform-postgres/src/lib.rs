use application::transaction::{Transaction, TransactionError, UnitOfWork};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool, Postgres};
use std::fmt;
use std::time::Duration;

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
    pub fn new(
        host: String,
        port: u16,
        database: String,
        username: String,
        password: String,
        max_connections: u32,
    ) -> Result<Self, PostgresPoolConfigError> {
        if max_connections == 0 {
            return Err(PostgresPoolConfigError::ZeroMaxConnections);
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

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub const fn max_connections(&self) -> u32 {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PostgresPoolConfigError {
    #[error("Postgres max connections must be greater than zero")]
    ZeroMaxConnections,
}

#[derive(thiserror::Error, Debug)]
pub enum PostgresConnectError {
    #[error("failed to connect to Postgres")]
    Connect(#[source] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct SqlxUnitOfWork {
    pool: PgPool,
}

pub struct SqlxTransaction {
    transaction: sqlx::Transaction<'static, Postgres>,
}

impl SqlxUnitOfWork {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SqlxTransaction {
    pub fn connection(&mut self) -> &mut PgConnection {
        &mut self.transaction
    }
}

#[async_trait::async_trait]
impl UnitOfWork for SqlxUnitOfWork {
    type Tx = SqlxTransaction;

    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        self.pool
            .begin()
            .await
            .map(|transaction| SqlxTransaction { transaction })
            .map_err(|_| TransactionError::BeginFailed)
    }
}

#[async_trait::async_trait]
impl Transaction for SqlxTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        self.transaction
            .commit()
            .await
            .map_err(|_| TransactionError::CommitFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_zero_max_connections() {
        let config = PostgresPoolConfig::new(
            "localhost".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            0,
        );

        assert_eq!(Err(PostgresPoolConfigError::ZeroMaxConnections), config);
    }

    #[test]
    fn should_redact_password_in_debug_output() {
        let config = PostgresPoolConfig::new(
            "localhost".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "very-secret".to_owned(),
            2,
        );

        let output = match config {
            Ok(config) => format!("{config:?}"),
            Err(error) => format!("unexpected error: {error}"),
        };

        assert!(!output.contains("very-secret"));
        assert!(output.contains("<redacted>"));
    }
}
