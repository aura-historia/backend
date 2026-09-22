use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, log_cold_start, log_invocation_start, logging_config_from_env,
    required_config_from_env,
};
use platform_observability::init;
use platform_postgres::VersionedPostgresCredentials;
use platform_postgres_secretsmanager::load_versioned_postgres_credentials_from_secret_arn;
use serde::Serialize;
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use std::time::Instant;
use thiserror::Error;
use tracing::info;

const ADMIN_SECRET_ARN_ENV: &str = "POSTGRES_ADMIN_SECRET_ARN";
const RUNTIME_SECRET_ARN_ENV: &str = "POSTGRES_RUNTIME_SECRET_ARN";
const MIGRATION_SECRET_ARN_ENV: &str = "POSTGRES_MIGRATION_SECRET_ARN";
const REPLICATION_SECRET_ARN_ENV: &str = "POSTGRES_REPLICATION_SECRET_ARN";
const INITIALIZATION_ADVISORY_LOCK: &str = "aura_historia_database_initialize";
const ROLE_BOOTSTRAP_SQL: &str = include_str!("../../../infra/sql/rds-bootstrap-roles-core.sql");
static ROOT_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Clone)]
struct MigrationConfig {
    postgres: LambdaPostgresConfig,
    admin_secret_arn: String,
    runtime_secret_arn: String,
    migration_secret_arn: String,
    replication_secret_arn: String,
}

impl MigrationConfig {
    fn from_env() -> Result<Self, InitializationError> {
        Ok(Self {
            postgres: LambdaPostgresConfig::from_env()
                .map_err(|_| InitializationError::InvalidConfiguration)?,
            admin_secret_arn: required_config_from_env(ADMIN_SECRET_ARN_ENV)
                .map_err(|_| InitializationError::InvalidConfiguration)?,
            runtime_secret_arn: required_config_from_env(RUNTIME_SECRET_ARN_ENV)
                .map_err(|_| InitializationError::InvalidConfiguration)?,
            migration_secret_arn: required_config_from_env(MIGRATION_SECRET_ARN_ENV)
                .map_err(|_| InitializationError::InvalidConfiguration)?,
            replication_secret_arn: required_config_from_env(REPLICATION_SECRET_ARN_ENV)
                .map_err(|_| InitializationError::InvalidConfiguration)?,
        })
    }
}

struct RoleCredentials {
    admin: VersionedPostgresCredentials,
    runtime: VersionedPostgresCredentials,
    migrator: VersionedPostgresCredentials,
    replication: VersionedPostgresCredentials,
}

impl RoleCredentials {
    async fn load(config: &MigrationConfig) -> Result<Self, InitializationError> {
        let admin = load_versioned_postgres_credentials_from_secret_arn(&config.admin_secret_arn)
            .await
            .map_err(|_| InitializationError::CredentialsUnavailable)?;
        let runtime =
            load_versioned_postgres_credentials_from_secret_arn(&config.runtime_secret_arn)
                .await
                .map_err(|_| InitializationError::CredentialsUnavailable)?;
        let migrator =
            load_versioned_postgres_credentials_from_secret_arn(&config.migration_secret_arn)
                .await
                .map_err(|_| InitializationError::CredentialsUnavailable)?;
        let replication =
            load_versioned_postgres_credentials_from_secret_arn(&config.replication_secret_arn)
                .await
                .map_err(|_| InitializationError::CredentialsUnavailable)?;

        Ok(Self {
            admin,
            runtime,
            migrator,
            replication,
        })
    }
}

#[derive(Debug, Error)]
enum InitializationError {
    #[error("database initialization configuration is invalid")]
    InvalidConfiguration,
    #[error("database initialization credentials are unavailable")]
    CredentialsUnavailable,
    #[error("database initialization connection failed")]
    Connection,
    #[error("database initialization is already running")]
    AlreadyRunning,
    #[error("database role bootstrap failed")]
    RoleBootstrap,
    #[error("database schema migration failed")]
    Migration,
    #[error("database initialization lock release failed")]
    LockRelease,
}

#[derive(Serialize)]
struct InitializationResult {
    status: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let config = MigrationConfig::from_env()?;
    log_cold_start("database-migration-lambda", initialization_started_at);

    run(service_fn(move |event: LambdaEvent<Value>| {
        let config = config.clone();
        async move {
            log_invocation_start("database-migration-lambda", &event.context);
            initialize_database(&config)
                .await
                .map_err(|error| -> Error { Box::new(error) })?;
            info!(
                event = "database.initialization.completed",
                "Database initialization completed"
            );
            Ok::<_, Error>(InitializationResult { status: "ready" })
        }
    }))
    .await
}

async fn initialize_database(config: &MigrationConfig) -> Result<(), InitializationError> {
    let credentials = RoleCredentials::load(config).await?;
    let mut admin_connection = config
        .postgres
        .migration_pool_config(credentials.admin.credentials())
        .map_err(|_| InitializationError::InvalidConfiguration)?
        .connect_connection()
        .await
        .map_err(|_| InitializationError::Connection)?;

    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
        .bind(INITIALIZATION_ADVISORY_LOCK)
        .fetch_one(&mut admin_connection)
        .await
        .map_err(|_| InitializationError::Connection)?;
    if !locked {
        return Err(InitializationError::AlreadyRunning);
    }

    bootstrap_roles(&mut admin_connection, &credentials).await?;
    apply_root_migrations(config, &credentials.migrator).await?;

    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(INITIALIZATION_ADVISORY_LOCK)
        .fetch_one(&mut admin_connection)
        .await
        .map_err(|_| InitializationError::LockRelease)?;
    if !unlocked {
        return Err(InitializationError::LockRelease);
    }

    Ok(())
}

async fn bootstrap_roles(
    connection: &mut PgConnection,
    credentials: &RoleCredentials,
) -> Result<(), InitializationError> {
    bootstrap_roles_inner(connection, credentials)
        .await
        .map_err(|_| InitializationError::RoleBootstrap)
}

async fn bootstrap_roles_inner(
    connection: &mut PgConnection,
    credentials: &RoleCredentials,
) -> Result<(), sqlx::Error> {
    let mut transaction = connection.begin().await?;

    sqlx::query(
        "SELECT set_config('aura.bootstrap.runtime_password', $1, true), \
                set_config('aura.bootstrap.migrator_password', $2, true), \
                set_config('aura.bootstrap.replication_password', $3, true)",
    )
    .bind(credentials.runtime.credentials().password())
    .bind(credentials.migrator.credentials().password())
    .bind(credentials.replication.credentials().password())
    .execute(&mut *transaction)
    .await?;

    sqlx::raw_sql(ROLE_BOOTSTRAP_SQL)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await
}

async fn apply_root_migrations(
    config: &MigrationConfig,
    migrator: &VersionedPostgresCredentials,
) -> Result<(), InitializationError> {
    let mut connection = config
        .postgres
        .migration_pool_config(migrator.credentials())
        .map_err(|_| InitializationError::InvalidConfiguration)?
        .connect_connection()
        .await
        .map_err(|_| InitializationError::Connection)?;

    ROOT_MIGRATIONS
        .run(&mut connection)
        .await
        .map_err(|_| InitializationError::Migration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_embed_plain_sql_role_bootstrap_without_psql_commands() {
        assert!(ROLE_BOOTSTRAP_SQL.contains("aura.bootstrap.runtime_password"));
        assert!(ROLE_BOOTSTRAP_SQL.contains("CREATE EXTENSION IF NOT EXISTS pg_trgm"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("\\set"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("\\i"));
    }

    #[test]
    fn should_return_only_safe_initialization_result() {
        let response = match serde_json::to_string(&InitializationResult { status: "ready" }) {
            Ok(response) => response,
            Err(error) => panic!("failed to serialize initialization result: {error}"),
        };

        assert_eq!(response, r#"{"status":"ready"}"#);
        assert!(!response.contains("password"));
        assert!(!response.contains("secret"));
    }

    #[tokio::test]
    async fn should_bootstrap_roles_and_apply_root_schema_as_migrator() {
        let pool = test_api::get_postgres_client().await;
        let mut connection = match pool.acquire().await {
            Ok(connection) => connection,
            Err(error) => panic!("failed to acquire PostgreSQL test connection: {error}"),
        };
        let create_rds_replication_role = sqlx::raw_sql(
            "DO $$ BEGIN\n                 IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'rds_replication') THEN\n                   CREATE ROLE rds_replication NOLOGIN;\n                 END IF;\n               END $$;",
        )
        .execute(&mut *connection)
        .await;
        if let Err(error) = create_rds_replication_role {
            panic!("failed to create RDS replication test role: {error}");
        }
        let credentials = RoleCredentials {
            admin: credentials("aura_admin", "admin-password"),
            runtime: credentials("aura_runtime", "runtime-password"),
            migrator: credentials("aura_migrator", "migrator-password"),
            replication: credentials("aura_replication", "replication-password"),
        };

        if let Err(error) = bootstrap_roles_inner(&mut connection, &credentials).await {
            panic!("role bootstrap failed: {error}");
        }
        let set_role = sqlx::query("SET ROLE aura_migrator")
            .execute(&mut *connection)
            .await;
        if let Err(error) = set_role {
            panic!("failed to assume migrator role: {error}");
        }
        if let Err(error) = ROOT_MIGRATIONS.run(&mut *connection).await {
            panic!("root migration failed as migrator: {error}");
        }
        let reset_role = sqlx::query("RESET ROLE").execute(&mut *connection).await;
        if let Err(error) = reset_role {
            panic!("failed to reset PostgreSQL role: {error}");
        }

        let schema_owner: String = match sqlx::query_scalar(
            "SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'public'",
        )
        .fetch_one(&mut *connection)
        .await
        {
            Ok(owner) => owner,
            Err(error) => panic!("failed to read public schema owner: {error}"),
        };
        let migration_version: i64 = match sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE version = 20260725090000",
        )
        .fetch_one(&mut *connection)
        .await
        {
            Ok(version) => version,
            Err(error) => panic!("failed to read root migration ledger: {error}"),
        };
        let runtime_can_insert: bool = match sqlx::query_scalar(
            "SELECT has_table_privilege('aura_runtime', 'public.users', 'INSERT')",
        )
        .fetch_one(&mut *connection)
        .await
        {
            Ok(value) => value,
            Err(error) => panic!("failed to check runtime role grant: {error}"),
        };
        let replication_can_select: bool = match sqlx::query_scalar(
            "SELECT has_table_privilege('aura_replication', 'public.users', 'SELECT')",
        )
        .fetch_one(&mut *connection)
        .await
        {
            Ok(value) => value,
            Err(error) => panic!("failed to check replication role grant: {error}"),
        };

        assert_eq!(schema_owner, "aura_migrator");
        assert_eq!(migration_version, 20260725090000);
        assert!(runtime_can_insert);
        assert!(replication_can_select);
    }

    fn credentials(username: &str, password: &str) -> VersionedPostgresCredentials {
        match VersionedPostgresCredentials::new(
            format!("{username}-version"),
            username.to_owned(),
            password.to_owned(),
        ) {
            Ok(credentials) => credentials,
            Err(error) => panic!("failed to create test credentials: {error}"),
        }
    }
}
