use application::transaction::{Transaction, TransactionError, UnitOfWork};
use async_trait::async_trait;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::{Connection, PgConnection, PgPool, Postgres};
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const DEFAULT_ACQUIRE_TIMEOUT_SECONDS: u64 = 5;
const LAMBDA_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);
const LAMBDA_STATEMENT_TIMEOUT: Duration = Duration::from_secs(5);
const LAMBDA_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const LAMBDA_IDLE_IN_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(10);
const LAMBDA_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const LAMBDA_MAX_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Clone, PartialEq, Eq)]
pub struct PostgresCredentials {
    username: String,
    password: String,
}

impl PostgresCredentials {
    pub fn new(username: String, password: String) -> Result<Self, PostgresCredentialsError> {
        if username.trim().is_empty() {
            return Err(PostgresCredentialsError::EmptyUsername);
        }
        if password.is_empty() {
            return Err(PostgresCredentialsError::EmptyPassword);
        }
        Ok(Self { username, password })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for PostgresCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PostgresCredentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PostgresCredentialsError {
    #[error("PostgreSQL credential username must not be empty")]
    EmptyUsername,
    #[error("PostgreSQL credential password must not be empty")]
    EmptyPassword,
}

#[derive(Clone)]
pub struct VersionedPostgresCredentials {
    version_id: String,
    credentials: PostgresCredentials,
}

impl VersionedPostgresCredentials {
    pub fn new(
        version_id: String,
        username: String,
        password: String,
    ) -> Result<Self, VersionedPostgresCredentialsError> {
        if version_id.trim().is_empty() {
            return Err(VersionedPostgresCredentialsError::EmptyVersionId);
        }
        let credentials = PostgresCredentials::new(username, password)
            .map_err(VersionedPostgresCredentialsError::Credentials)?;
        Ok(Self {
            version_id,
            credentials,
        })
    }

    pub fn version_id(&self) -> &str {
        &self.version_id
    }

    pub fn credentials(&self) -> &PostgresCredentials {
        &self.credentials
    }
}

impl fmt::Debug for VersionedPostgresCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VersionedPostgresCredentials")
            .field("version_id", &self.version_id)
            .field("credentials", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VersionedPostgresCredentialsError {
    #[error("PostgreSQL secret version must not be empty")]
    EmptyVersionId,
    #[error("invalid PostgreSQL credentials")]
    Credentials(#[source] PostgresCredentialsError),
}

#[derive(Debug, thiserror::Error)]
#[error("PostgreSQL credential refresh unavailable")]
pub struct PostgresCredentialRefreshError;

#[async_trait]
pub trait PostgresCredentialsProvider: Send + Sync {
    async fn current(&self)
    -> Result<VersionedPostgresCredentials, PostgresCredentialRefreshError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostgresTlsConfig {
    VerifyFull { root_certificate: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostgresPoolTimeouts {
    acquire: Duration,
    statement: Duration,
    lock: Duration,
    idle_in_transaction: Duration,
}

impl PostgresPoolTimeouts {
    pub const fn lambda() -> Self {
        Self {
            acquire: LAMBDA_ACQUIRE_TIMEOUT,
            statement: LAMBDA_STATEMENT_TIMEOUT,
            lock: LAMBDA_LOCK_TIMEOUT,
            idle_in_transaction: LAMBDA_IDLE_IN_TRANSACTION_TIMEOUT,
        }
    }

    pub const fn acquire(&self) -> Duration {
        self.acquire
    }

    pub const fn statement(&self) -> Duration {
        self.statement
    }

    pub const fn lock(&self) -> Duration {
        self.lock
    }

    pub const fn idle_in_transaction(&self) -> Duration {
        self.idle_in_transaction
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PostgresPoolConfig {
    host: String,
    port: u16,
    database: String,
    username: String,
    password: String,
    max_connections: u32,
    tls: Option<PostgresTlsConfig>,
    timeouts: Option<PostgresPoolTimeouts>,
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
            .field("tls", &self.tls)
            .field("timeouts", &self.timeouts)
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
            tls: None,
            timeouts: None,
        })
    }

    pub fn lambda(
        host: String,
        port: u16,
        database: String,
        username: String,
        password: String,
        max_connections: u32,
        root_certificate: PathBuf,
    ) -> Result<Self, PostgresPoolConfigError> {
        if root_certificate.as_os_str().is_empty() {
            return Err(PostgresPoolConfigError::EmptyRootCertificate);
        }

        let mut config = Self::new(host, port, database, username, password, max_connections)?;
        config.tls = Some(PostgresTlsConfig::VerifyFull { root_certificate });
        config.timeouts = Some(PostgresPoolTimeouts::lambda());
        Ok(config)
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

    pub const fn min_connections(&self) -> u32 {
        0
    }

    pub const fn timeouts(&self) -> Option<PostgresPoolTimeouts> {
        self.timeouts
    }

    pub fn tls(&self) -> Option<&PostgresTlsConfig> {
        self.tls.as_ref()
    }

    pub fn connect_options(&self) -> PgConnectOptions {
        let options = PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.database)
            .username(&self.username)
            .password(&self.password);

        match &self.tls {
            Some(PostgresTlsConfig::VerifyFull { root_certificate }) => options
                .ssl_mode(PgSslMode::VerifyFull)
                .ssl_root_cert(root_certificate),
            None => options,
        }
    }

    pub fn pool_options(&self) -> PgPoolOptions {
        let options = PgPoolOptions::new()
            .min_connections(self.min_connections())
            .max_connections(self.max_connections);

        let Some(timeouts) = self.timeouts else {
            return options.acquire_timeout(Duration::from_secs(DEFAULT_ACQUIRE_TIMEOUT_SECONDS));
        };

        let statement_timeout = duration_setting(timeouts.statement);
        let lock_timeout = duration_setting(timeouts.lock);
        let idle_in_transaction_timeout = duration_setting(timeouts.idle_in_transaction);
        let max_connections = self.max_connections;

        options
            .acquire_timeout(timeouts.acquire)
            .idle_timeout(LAMBDA_IDLE_TIMEOUT)
            .max_lifetime(LAMBDA_MAX_LIFETIME)
            .test_before_acquire(true)
            .after_connect(move |connection, _| {
                let statement_timeout = statement_timeout.clone();
                let lock_timeout = lock_timeout.clone();
                let idle_in_transaction_timeout = idle_in_transaction_timeout.clone();
                Box::pin(async move {
                    sqlx::query(
                        "SELECT set_config('statement_timeout', $1, false), set_config('lock_timeout', $2, false), set_config('idle_in_transaction_session_timeout', $3, false)",
                    )
                    .bind(statement_timeout)
                    .bind(lock_timeout)
                    .bind(idle_in_transaction_timeout)
                    .execute(connection)
                    .await?;
                    info!(
                        metric = "postgres_pool_connection_opened",
                        postgres_pool_max_connections = max_connections,
                        "Postgres pool connection opened or reconnected"
                    );
                    Ok(())
                })
            })
    }

    pub async fn connect(&self) -> Result<PgPool, sqlx::Error> {
        Ok(self
            .pool_options()
            .connect_lazy_with(self.connect_options()))
    }

    pub async fn connect_connection(&self) -> Result<PgConnection, sqlx::Error> {
        let options = self.direct_connection_options();
        let connect = PgConnection::connect_with(&options);

        let connection = match self.direct_connection_timeout() {
            Some(timeout) => sqlx::__rt::timeout(timeout, connect)
                .await
                .map_err(|_| sqlx::Error::PoolTimedOut)??,
            None => connect.await?,
        };
        Ok(connection)
    }

    fn direct_connection_options(&self) -> PgConnectOptions {
        let Some(timeouts) = self.timeouts else {
            return self.connect_options();
        };

        self.connect_options().options([
            ("statement_timeout", duration_setting(timeouts.statement)),
            ("lock_timeout", duration_setting(timeouts.lock)),
            (
                "idle_in_transaction_session_timeout",
                duration_setting(timeouts.idle_in_transaction),
            ),
        ])
    }

    fn direct_connection_timeout(&self) -> Option<Duration> {
        self.timeouts.map(|timeouts| timeouts.acquire)
    }
}

fn duration_setting(duration: Duration) -> String {
    format!("{}ms", duration.as_millis())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PostgresPoolConfigError {
    #[error("Postgres max connections must be greater than zero")]
    ZeroMaxConnections,
    #[error("Postgres TLS root certificate path must not be empty")]
    EmptyRootCertificate,
}

#[derive(thiserror::Error, Debug)]
pub enum PostgresConnectError {
    #[error("failed to connect to Postgres")]
    Connect,
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
        let started_at = Instant::now();
        match self.pool.begin().await {
            Ok(transaction) => {
                info!(
                    metric = "postgres_pool_acquire",
                    outcome = "success",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "Postgres transaction connection acquired"
                );
                Ok(SqlxTransaction { transaction })
            }
            Err(_) => {
                warn!(
                    metric = "postgres_pool_acquire",
                    outcome = "failure",
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "Postgres transaction connection acquisition failed"
                );
                Err(TransactionError::BeginFailed)
            }
        }
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

    #[test]
    fn should_configure_lambda_pool_with_verified_tls_and_bounded_waits() {
        let config = PostgresPoolConfig::lambda(
            "database.example.test".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            1,
            PathBuf::from("/opt/aura-historia/rds-ca.pem"),
        );

        let config = match config {
            Ok(config) => config,
            Err(error) => panic!("unexpected config error: {error}"),
        };

        assert_eq!(config.min_connections(), 0);
        assert_eq!(config.max_connections(), 1);
        assert_eq!(config.timeouts(), Some(PostgresPoolTimeouts::lambda()));
        assert_eq!(
            config.tls(),
            Some(&PostgresTlsConfig::VerifyFull {
                root_certificate: PathBuf::from("/opt/aura-historia/rds-ca.pem"),
            })
        );
        assert!(format!("{:?}", config.connect_options()).contains("VerifyFull"));
    }

    #[test]
    fn should_bound_direct_lambda_connection_with_startup_timeout_configuration() {
        let lambda = PostgresPoolConfig::lambda(
            "database.example.test".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            1,
            PathBuf::from("/opt/aura-historia/rds-ca.pem"),
        );
        let lambda = match lambda {
            Ok(config) => config,
            Err(error) => panic!("unexpected config error: {error}"),
        };
        let direct_options = lambda.direct_connection_options();
        let startup_options = direct_options.get_options().unwrap_or_default();
        let native = PostgresPoolConfig::new(
            "localhost".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            1,
        );
        let native = match native {
            Ok(config) => config,
            Err(error) => panic!("unexpected config error: {error}"),
        };

        assert_eq!(
            Some(LAMBDA_ACQUIRE_TIMEOUT),
            lambda.direct_connection_timeout()
        );
        assert!(startup_options.contains("statement_timeout=5000ms"));
        assert!(startup_options.contains("lock_timeout=1000ms"));
        assert!(startup_options.contains("idle_in_transaction_session_timeout=10000ms"));
        assert_eq!(None, lambda.connect_options().get_options());
        assert_eq!(None, native.direct_connection_timeout());
        assert_eq!(None, native.direct_connection_options().get_options());
    }

    #[test]
    fn should_reject_empty_lambda_root_certificate_path() {
        let config = PostgresPoolConfig::lambda(
            "localhost".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            1,
            PathBuf::new(),
        );

        assert_eq!(Err(PostgresPoolConfigError::EmptyRootCertificate), config);
    }

    #[tokio::test]
    async fn should_create_lambda_pool_without_opening_a_connection() {
        let config = PostgresPoolConfig::lambda(
            "unreachable.example.test".to_owned(),
            5432,
            "aura".to_owned(),
            "postgres".to_owned(),
            "secret".to_owned(),
            1,
            PathBuf::from("/opt/aura-historia/rds-ca.pem"),
        );
        let config = match config {
            Ok(config) => config,
            Err(error) => panic!("unexpected config error: {error}"),
        };
        let pool = match config.connect().await {
            Ok(pool) => pool,
            Err(error) => panic!("unexpected pool error: {error}"),
        };

        assert_eq!(pool.size(), 0);
        assert_eq!(pool.num_idle(), 0);
    }
}
