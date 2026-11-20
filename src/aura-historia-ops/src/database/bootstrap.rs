//! First-time role bootstrap. The administrator creates the runtime, migrator and inspector roles
//! and the migrator applies every root migration, under one initialization lock.

use super::{MIGRATOR_PASSWORD_ENV, ROOT_MIGRATIONS};
use crate::OpsError;
use application::error::BoxError;
use platform_postgres::PostgresCredentials;
use platform_process::postgres::PostgresTarget;
use sqlx::{Connection, PgConnection};
use thiserror::Error;

const ADMIN_USERNAME_ENV: &str = "POSTGRES_ADMIN_USERNAME";
const ADMIN_PASSWORD_ENV: &str = "POSTGRES_ADMIN_PASSWORD";
const RUNTIME_PASSWORD_ENV: &str = "POSTGRES_RUNTIME_PASSWORD";
const INSPECTOR_PASSWORD_ENV: &str = "POSTGRES_INSPECTOR_PASSWORD";
/// Optional: without it the crawler's ingestion role exists but cannot log in.
const CRAWLER_INGESTION_PASSWORD_ENV: &str = "POSTGRES_CRAWLER_INGESTION_PASSWORD";
/// The role the bootstrap SQL creates for migrations.
const MIGRATOR_ROLE: &str = "aura_migrator";
const INITIALIZATION_ADVISORY_LOCK: &str = "aura_historia_database_initialize";
const ROLE_BOOTSTRAP_SQL: &str =
    include_str!("../../../../infra/sql/host-bootstrap-roles-core.sql");
/// Drops the replication placeholder the role bootstrap created for the initial schema, once
/// the DMS retirement migration removed what it was granted, revoking what a database restored
/// from RDS still grants it. A role still referenced by another database of the cluster is left
/// alone; one this administrator may not drop is kept with a warning, as the retirement
/// migration does.
const RETIRE_REPLICATION_PLACEHOLDER_SQL: &str = r#"
DO $$
DECLARE
    replication oid := (SELECT oid FROM pg_roles WHERE rolname = 'aura_replication');
BEGIN
    IF replication IS NULL OR EXISTS (
        SELECT 1
        FROM pg_shdepend
        WHERE refclassid = 'pg_authid'::regclass
          AND refobjid = replication
          AND dbid NOT IN (0, (SELECT oid FROM pg_database WHERE datname = current_database()))
    ) THEN
        RETURN;
    END IF;
    BEGIN
        DROP ROLE aura_replication;
    EXCEPTION
        WHEN dependent_objects_still_exist THEN
            -- Privileges restored from the RDS dump still name the role. Revoking them takes the
            -- role's privileges, which its creator's ADMIN option lets the administrator grant
            -- itself; the administrator is a member of the owner that granted them.
            EXECUTE format('GRANT aura_replication TO %I', current_user);
            DROP OWNED BY aura_replication;
            DROP ROLE aura_replication;
    END;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE WARNING 'aura_replication was kept: dropping it needs an administrator';
    WHEN dependent_objects_still_exist THEN
        RAISE WARNING 'aura_replication was kept: it still owns a shared object';
END
$$;
"#;

pub(super) struct BootstrapConfig {
    target: PostgresTarget,
    credentials: RoleCredentials,
}

impl BootstrapConfig {
    pub(super) fn from_env() -> Result<Self, OpsError> {
        let required = super::required;
        Ok(Self {
            target: super::target()?,
            credentials: RoleCredentials {
                admin: PostgresCredentials::new(
                    required(ADMIN_USERNAME_ENV)?,
                    required(ADMIN_PASSWORD_ENV)?,
                )
                .map_err(|error| OpsError::Config(error.into()))?,
                runtime_password: required(RUNTIME_PASSWORD_ENV)?,
                migrator_password: required(MIGRATOR_PASSWORD_ENV)?,
                inspector_password: required(INSPECTOR_PASSWORD_ENV)?,
                crawler_ingestion_password: platform_process::config::optional(
                    CRAWLER_INGESTION_PASSWORD_ENV,
                    String::new(),
                )
                .map_err(|error| OpsError::Config(error.into()))?,
            },
        })
    }
}

/// The administrator's credentials and the passwords the bootstrap sets on the application
/// roles. No `Debug`: it holds secrets.
struct RoleCredentials {
    admin: PostgresCredentials,
    runtime_password: String,
    migrator_password: String,
    inspector_password: String,
    /// Empty when the crawler's role is not to log in yet.
    crawler_ingestion_password: String,
}

pub(super) struct BootstrapOutcome {
    /// The replication placeholder survived because this administrator may not drop it or it
    /// still owns a shared object; the operator drops it.
    pub replication_role_kept: bool,
}

#[derive(Debug, Error)]
pub(super) enum InitializationError {
    #[error("database initialization configuration is invalid")]
    InvalidConfiguration(#[source] BoxError),
    #[error("database initialization connection failed")]
    Connection(#[source] BoxError),
    #[error("database initialization is already running")]
    AlreadyRunning,
    #[error("database role bootstrap failed")]
    RoleBootstrap(#[source] BoxError),
    #[error("database schema migration failed")]
    Migration(#[source] BoxError),
    #[error("database initialization lock release failed")]
    LockRelease(#[source] BoxError),
    #[error("database initialization lock was not held")]
    LockNotHeld,
}

pub(super) async fn initialize_database(
    config: &BootstrapConfig,
) -> Result<BootstrapOutcome, InitializationError> {
    let admin = &config.credentials.admin;
    let mut admin_connection = connect(
        &config.target,
        admin.username().to_owned(),
        admin.password().to_owned(),
    )
    .await?;

    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
        .bind(INITIALIZATION_ADVISORY_LOCK)
        .fetch_one(&mut admin_connection)
        .await
        .map_err(|source| InitializationError::Connection(Box::new(source)))?;
    if !locked {
        return Err(InitializationError::AlreadyRunning);
    }

    bootstrap_roles(&mut admin_connection, &config.credentials).await?;
    apply_root_migrations(&config.target, &config.credentials.migrator_password).await?;
    let replication_role_kept = retire_replication_placeholder(&mut admin_connection)
        .await
        .map_err(|source| InitializationError::RoleBootstrap(Box::new(source)))?;

    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(INITIALIZATION_ADVISORY_LOCK)
        .fetch_one(&mut admin_connection)
        .await
        .map_err(|source| InitializationError::LockRelease(Box::new(source)))?;
    if !unlocked {
        return Err(InitializationError::LockNotHeld);
    }

    Ok(BootstrapOutcome {
        replication_role_kept,
    })
}

async fn connect(
    target: &PostgresTarget,
    username: String,
    password: String,
) -> Result<PgConnection, InitializationError> {
    target
        .migration_config(username, password)
        .map_err(|source| InitializationError::InvalidConfiguration(Box::new(source)))?
        .connect_connection()
        .await
        .map_err(|source| InitializationError::Connection(Box::new(source)))
}

async fn bootstrap_roles(
    connection: &mut PgConnection,
    credentials: &RoleCredentials,
) -> Result<(), InitializationError> {
    bootstrap_roles_inner(connection, credentials)
        .await
        .map_err(|source| InitializationError::RoleBootstrap(Box::new(source)))
}

async fn bootstrap_roles_inner(
    connection: &mut PgConnection,
    credentials: &RoleCredentials,
) -> Result<(), sqlx::Error> {
    let mut transaction = connection.begin().await?;

    sqlx::query(
        "SELECT set_config('aura.bootstrap.runtime_password', $1, true), \
                set_config('aura.bootstrap.migrator_password', $2, true), \
                set_config('aura.bootstrap.inspection_password', $3, true), \
                set_config('aura.bootstrap.crawler_ingestion_password', $4, true)",
    )
    .bind(&credentials.runtime_password)
    .bind(&credentials.migrator_password)
    .bind(&credentials.inspector_password)
    .bind(&credentials.crawler_ingestion_password)
    .execute(&mut *transaction)
    .await?;

    sqlx::raw_sql(ROLE_BOOTSTRAP_SQL)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await
}

async fn apply_root_migrations(
    target: &PostgresTarget,
    migrator_password: &str,
) -> Result<(), InitializationError> {
    let mut connection = connect(
        target,
        MIGRATOR_ROLE.to_owned(),
        migrator_password.to_owned(),
    )
    .await?;

    ROOT_MIGRATIONS
        .run(&mut connection)
        .await
        .map_err(|source| InitializationError::Migration(Box::new(source)))?;
    sqlx::query(
        "GRANT SELECT (version, checksum, success) ON public._sqlx_migrations TO aura_inspector",
    )
    .execute(&mut connection)
    .await
    .map_err(|source| InitializationError::RoleBootstrap(Box::new(source)))?;
    Ok(())
}

/// Returns whether the placeholder is still there.
async fn retire_replication_placeholder(
    connection: &mut PgConnection,
) -> Result<bool, sqlx::Error> {
    sqlx::raw_sql(RETIRE_REPLICATION_PLACEHOLDER_SQL)
        .execute(&mut *connection)
        .await?;
    sqlx::query_scalar("SELECT to_regrole('aura_replication') IS NOT NULL")
        .fetch_one(connection)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_embed_unique_root_migration_versions() {
        let mut versions = std::collections::BTreeSet::new();
        for migration in ROOT_MIGRATIONS.iter() {
            assert!(
                versions.insert(migration.version),
                "duplicate root migration version: {}",
                migration.version
            );
        }
    }

    #[test]
    fn should_embed_plain_sql_role_bootstrap_without_psql_commands_or_replication() {
        assert!(ROLE_BOOTSTRAP_SQL.contains("aura.bootstrap.runtime_password"));
        assert!(ROLE_BOOTSTRAP_SQL.contains("CREATE EXTENSION IF NOT EXISTS pg_trgm"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("\\set"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("\\i"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("rds_replication"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("replication_password"));
        assert!(!ROLE_BOOTSTRAP_SQL.contains("aura_admin"));
    }

    #[tokio::test]
    async fn should_bootstrap_roles_and_apply_root_schema_as_migrator() {
        let reservation = test_api::reserve_exclusive()
            .await
            .expect("reserve raw PostgreSQL fixture");
        reservation
            .scope(async {
                let pool = test_api::get_postgres_client().await;
                let mut connection = pool.acquire().await.expect("acquire PostgreSQL test connection");
                // A non-superuser administrator that owns the database and may create roles.
                sqlx::raw_sql(
                    "DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION pg_database_owner; GRANT USAGE ON SCHEMA public TO PUBLIC; DO $$ BEGIN\n                 IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_admin') THEN\n                   CREATE ROLE aura_admin LOGIN NOSUPERUSER NOCREATEDB CREATEROLE NOINHERIT;\n                 ELSE\n                   ALTER ROLE aura_admin LOGIN NOSUPERUSER NOCREATEDB CREATEROLE NOINHERIT;\n                 END IF;\n               END $$;\n               ALTER DATABASE postgres OWNER TO aura_admin;",
                )
                .execute(&mut *connection)
                .await
                .expect("prepare simulated administrator");
                let credentials = RoleCredentials {
                    admin: PostgresCredentials::new("aura_admin".into(), "admin-password".into())
                        .expect("administrator credentials"),
                    runtime_password: "runtime-password".into(),
                    migrator_password: "migrator-password".into(),
                    inspector_password: "inspection-password".into(),
                    crawler_ingestion_password: String::new(),
                };

                sqlx::query("SET ROLE aura_admin")
                    .execute(&mut *connection)
                    .await
                    .expect("assume simulated administrator");
                let executor_is_superuser: bool =
                    sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = current_user")
                        .fetch_one(&mut *connection)
                        .await
                        .expect("read bootstrap executor attributes");
                let migrator_absent_before_bootstrap: bool =
                    sqlx::query_scalar("SELECT to_regrole('aura_migrator') IS NULL")
                        .fetch_one(&mut *connection)
                        .await
                        .expect("read migrator state before bootstrap");
                assert!(!executor_is_superuser);
                assert!(migrator_absent_before_bootstrap);

                bootstrap_roles_inner(&mut connection, &credentials)
                    .await
                    .expect("role bootstrap as simulated administrator");
                let migrator_can_create_database: bool = sqlx::query_scalar(
                    "SELECT has_database_privilege('aura_migrator', current_database(), 'CREATE')",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("read migrator database privilege");
                let administrator_can_set_migrator: bool =
                    sqlx::query_scalar("SELECT pg_has_role('aura_admin', 'aura_migrator', 'MEMBER')")
                        .fetch_one(&mut *connection)
                        .await
                        .expect("read administrator membership");
                assert!(migrator_can_create_database);
                assert!(administrator_can_set_migrator);

                sqlx::query("SET ROLE aura_migrator")
                    .execute(&mut *connection)
                    .await
                    .expect("assume migrator role");
                ROOT_MIGRATIONS
                    .run(&mut *connection)
                    .await
                    .expect("root migration as migrator");
                sqlx::query("RESET ROLE")
                    .execute(&mut *connection)
                    .await
                    .expect("reset role after migration");
                sqlx::query("SET ROLE aura_admin")
                    .execute(&mut *connection)
                    .await
                    .expect("reassume simulated administrator");
                retire_replication_placeholder(&mut connection)
                    .await
                    .expect("retire the replication placeholder or keep it with a warning");
                bootstrap_roles_inner(&mut connection, &credentials)
                    .await
                    .expect("repeat role bootstrap as simulated administrator");
                retire_replication_placeholder(&mut connection)
                    .await
                    .expect("retire the repeated replication placeholder");
                sqlx::query("RESET ROLE")
                    .execute(&mut *connection)
                    .await
                    .expect("reset role after repeat bootstrap");

                let schema_owner: String = sqlx::query_scalar(
                    "SELECT pg_get_userbyid(nspowner) FROM pg_namespace WHERE nspname = 'public'",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("read public schema owner");
                let users_owner: String = sqlx::query_scalar(
                    "SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid = 'public.users'::regclass",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("read users table owner");
                let migration_version: i64 = sqlx::query_scalar(
                    "SELECT version FROM _sqlx_migrations WHERE version = 20260725090000",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("read root migration ledger");
                let runtime_can_insert: bool = sqlx::query_scalar(
                    "SELECT has_table_privilege('aura_runtime', 'public.users', 'INSERT')",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("check runtime role grant");
                let heartbeat_schema_absent: bool =
                    sqlx::query_scalar("SELECT to_regnamespace('dms_heartbeat') IS NULL")
                        .fetch_one(&mut *connection)
                        .await
                        .expect("check retired heartbeat schema");

                assert_eq!(schema_owner, "aura_migrator");
                assert_eq!(users_owner, "aura_migrator");
                assert_eq!(migration_version, 20260725090000);
                assert!(runtime_can_insert);
                assert!(heartbeat_schema_absent);
                let no_replication_login: bool = sqlx::query_scalar(
                    "SELECT NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_replication' AND (rolcanlogin OR rolreplication))",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("check replication role attributes");
                assert!(no_replication_login);
                let crawler_privileges: (bool, bool, bool, bool) = sqlx::query_as(
                    "SELECT has_database_privilege('aura_crawler_ingestion', current_database(), 'CONNECT'), \
                            has_function_privilege('aura_crawler_ingestion', \
                                'public.job_queue_enqueue_product_listing_ingestion(text, text, text)', 'EXECUTE'), \
                            has_table_privilege('aura_crawler_ingestion', 'public.job_queue_jobs', 'SELECT, INSERT'), \
                            (SELECT rolcanlogin FROM pg_roles WHERE rolname = 'aura_crawler_ingestion')",
                )
                .fetch_one(&mut *connection)
                .await
                .expect("check crawler ingestion role grants");
                assert_eq!(
                    (true, true, false, false),
                    crawler_privileges,
                    "the crawler role connects and enqueues only, and logs in only with a password"
                );
                sqlx::query("SET ROLE aura_inspector").execute(&mut *connection).await.expect("assume inspector");
                sqlx::query("SELECT version, checksum, success FROM _sqlx_migrations").fetch_all(&mut *connection).await.expect("read sanitized ledger");
                assert!(sqlx::query("SELECT * FROM users").fetch_all(&mut *connection).await.is_err());
                assert!(sqlx::query("DELETE FROM _sqlx_migrations").execute(&mut *connection).await.is_err());
                sqlx::query("RESET ROLE").execute(&mut *connection).await.expect("reset inspector role");
            })
            .await;
    }
}
