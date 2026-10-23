use application::error::BoxError;
use platform_lambda_bootstrap::{LambdaPostgresConfig, required_config_from_env};
use platform_postgres_secretsmanager::load_versioned_postgres_credentials_from_secret_arn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, migrate::Migrate};
use std::{collections::BTreeMap, time::Duration};
use thiserror::Error;

const SCHEMA_SQL: &str = include_str!("../../../infra/sql/rds-inspect-schema.sql");
static ROOT_MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Clone)]
pub(super) struct OperationConfig {
    pub target: String,
    postgres: LambdaPostgresConfig,
    secret_arn: String,
}
impl OperationConfig {
    pub fn from_env(secret_name: &'static str) -> Result<Self, OperationError> {
        let target = required_config_from_env("DATABASE_TARGET").map_err(configuration)?;
        if !["dev", "prod"].contains(&target.as_str()) {
            return Err(OperationError::InvalidRequest);
        }
        Ok(Self {
            target,
            postgres: LambdaPostgresConfig::from_env().map_err(configuration)?,
            secret_arn: required_config_from_env(secret_name).map_err(configuration)?,
        })
    }
    async fn connect(&self) -> Result<PgConnection, OperationError> {
        let credentials = load_versioned_postgres_credentials_from_secret_arn(&self.secret_arn)
            .await
            .map_err(|source| OperationError::Credentials(Box::new(source)))?;
        let pool = self
            .postgres
            .migration_pool_config(credentials.credentials())
            .map_err(configuration)?;
        let mut connection = pool.connect_connection().await.map_err(database)?;
        sqlx::raw_sql("SET statement_timeout='120s'; SET lock_timeout='5s'; SET idle_in_transaction_session_timeout='120s'")
            .execute(&mut connection).await.map_err(database)?;
        Ok(connection)
    }
}
fn configuration(source: impl std::error::Error + Send + Sync + 'static) -> OperationError {
    OperationError::Configuration(Box::new(source))
}
fn database(source: sqlx::Error) -> OperationError {
    OperationError::Database(Box::new(source))
}

#[derive(Debug, Error)]
pub(super) enum OperationError {
    #[error("database operation configuration invalid")]
    Configuration(#[source] BoxError),
    #[error("database operation credential unavailable")]
    Credentials(#[source] BoxError),
    #[error("database operation failed; inspect actual ledger before retry")]
    Database(#[source] BoxError),
    #[error("database operation request invalid")]
    InvalidRequest,
    #[error("database ledger differs from approved migration history")]
    LedgerMismatch,
    #[error("pending nontransactional migration requires maintenance")]
    Nontransactional,
    #[error("database operation exceeded its bounded runtime; outcome unknown")]
    Timeout,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingMigration {
    pub version: i64,
    pub checksum: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct LedgerEntry {
    pub checksum: String,
    pub success: bool,
    pub version: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MigrationRequest {
    pub request_id: String,
    pub bundle_digest: String,
    pub expected_ledger_fingerprint: String,
    pub pending: Vec<PendingMigration>,
}
#[derive(Serialize)]
pub(super) struct InspectionResult {
    pub inspector_version: &'static str,
    pub target: String,
    pub ledger: Vec<LedgerEntry>,
    pub ledger_fingerprint: String,
    pub schema: Value,
}
#[derive(Serialize)]
pub(super) struct MigrationResult {
    status: &'static str,
    request_id: String,
    bundle_digest: String,
    target: String,
    applied: Vec<PendingMigration>,
    ledger: Vec<LedgerEntry>,
    ledger_fingerprint: String,
    operation_version: &'static str,
}
pub(super) fn fingerprint(ledger: &[LedgerEntry]) -> Result<String, OperationError> {
    let encoded =
        serde_json::to_vec(ledger).map_err(|source| OperationError::Database(Box::new(source)))?;
    Ok(hex(&Sha256::digest(encoded)))
}
async fn ledger(connection: &mut PgConnection) -> Result<Vec<LedgerEntry>, OperationError> {
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations') IS NOT NULL")
            .fetch_one(&mut *connection)
            .await
            .map_err(database)?;
    if !exists {
        return Ok(vec![]);
    }
    let rows: Vec<(i64, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT version, checksum, success FROM public._sqlx_migrations ORDER BY version",
    )
    .fetch_all(connection)
    .await
    .map_err(database)?;
    Ok(rows
        .into_iter()
        .map(|(version, checksum, success)| LedgerEntry {
            version,
            checksum: hex(&checksum),
            success,
        })
        .collect())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(super) async fn inspect(config: &OperationConfig) -> Result<InspectionResult, OperationError> {
    let mut connection = config.connect().await?;
    sqlx::raw_sql("BEGIN READ ONLY ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut connection)
        .await
        .map_err(database)?;
    let ledger = ledger(&mut connection).await?;
    let schema = sqlx::query_scalar(SCHEMA_SQL)
        .fetch_one(&mut connection)
        .await
        .map_err(database)?;
    sqlx::raw_sql("COMMIT")
        .execute(&mut connection)
        .await
        .map_err(database)?;
    Ok(InspectionResult {
        inspector_version: "database-inspector-v1",
        target: config.target.clone(),
        ledger_fingerprint: fingerprint(&ledger)?,
        ledger,
        schema,
    })
}
fn validate_plan(
    current: &[LedgerEntry],
    request: &MigrationRequest,
    migrations: &sqlx::migrate::Migrator,
) -> Result<Vec<PendingMigration>, OperationError> {
    if request.request_id.is_empty()
        || request.bundle_digest.len() != 64
        || !request
            .bundle_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || request.expected_ledger_fingerprint.len() != 64
    {
        return Err(OperationError::InvalidRequest);
    }
    let candidate: BTreeMap<_, _> = migrations
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| (m.version, m))
        .collect();
    for row in current {
        if !row.success
            || candidate
                .get(&row.version)
                .is_none_or(|migration| hex(&migration.checksum) != row.checksum)
        {
            return Err(OperationError::LedgerMismatch);
        }
    }
    let reviewed: BTreeMap<_, _> = request
        .pending
        .iter()
        .map(|migration| (migration.version, migration))
        .collect();
    if reviewed.len() != request.pending.len()
        || request
            .pending
            .windows(2)
            .any(|pair| pair[0].version >= pair[1].version)
    {
        return Err(OperationError::InvalidRequest);
    }
    // On an ambiguous retry only an exact prefix of this request may have committed.
    let initial: Vec<_> = current
        .iter()
        .filter(|row| !reviewed.contains_key(&row.version))
        .cloned()
        .collect();
    if fingerprint(&initial)? != request.expected_ledger_fingerprint {
        return Err(OperationError::LedgerMismatch);
    }
    let expected: Vec<_> = candidate
        .values()
        .filter(|migration| !initial.iter().any(|row| row.version == migration.version))
        .map(|migration| PendingMigration {
            version: migration.version,
            checksum: hex(&migration.checksum),
        })
        .collect();
    if expected != request.pending {
        return Err(OperationError::LedgerMismatch);
    }
    let committed: Vec<_> = request
        .pending
        .iter()
        .filter(|migration| current.iter().any(|row| row.version == migration.version))
        .cloned()
        .collect();
    if !request.pending.starts_with(&committed) {
        return Err(OperationError::LedgerMismatch);
    }
    let pending: Vec<_> = request
        .pending
        .iter()
        .filter(|migration| !current.iter().any(|row| row.version == migration.version))
        .cloned()
        .collect();
    if pending
        .iter()
        .any(|migration| candidate[&migration.version].no_tx)
    {
        return Err(OperationError::Nontransactional);
    }
    Ok(pending)
}
pub(super) async fn migrate(
    config: &OperationConfig,
    request: MigrationRequest,
) -> Result<MigrationResult, OperationError> {
    let mut connection = config.connect().await?;
    tokio::time::timeout(
        Duration::from_secs(600),
        migrate_connection(&mut connection, &config.target, request, &ROOT_MIGRATIONS),
    )
    .await
    .map_err(|_| OperationError::Timeout)?
}
async fn migrate_connection(
    connection: &mut PgConnection,
    target: &str,
    request: MigrationRequest,
    migrations: &sqlx::migrate::Migrator,
) -> Result<MigrationResult, OperationError> {
    connection
        .lock()
        .await
        .map_err(|source| OperationError::Database(Box::new(source)))?;
    let outcome = apply_locked(connection, &request, migrations).await;
    let unlocked = connection
        .unlock()
        .await
        .map_err(|source| OperationError::Database(Box::new(source)));
    let (pending, ledger) = outcome?;
    unlocked?;
    Ok(MigrationResult {
        status: if pending.is_empty() {
            "NO_OP"
        } else {
            "APPLIED"
        },
        request_id: request.request_id,
        bundle_digest: request.bundle_digest,
        target: target.to_owned(),
        applied: pending,
        ledger_fingerprint: fingerprint(&ledger)?,
        ledger,
        operation_version: "database-migrator-v1",
    })
}

async fn apply_locked(
    connection: &mut PgConnection,
    request: &MigrationRequest,
    migrations: &sqlx::migrate::Migrator,
) -> Result<(Vec<PendingMigration>, Vec<LedgerEntry>), OperationError> {
    let current = ledger(connection).await?;
    let pending = validate_plan(&current, request, migrations)?;
    // SQLx's lock remains held continuously from the ledger check through final inspection.
    let unlocked_migrator = sqlx::migrate::Migrator {
        locking: false,
        ..sqlx::migrate::Migrator::with_migrations(migrations.iter().cloned().collect())
    };
    unlocked_migrator
        .run_direct(None, connection, false)
        .await
        .map_err(|source| OperationError::Database(Box::new(source)))?;
    Ok((pending, ledger(connection).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{
        SqlSafeStr,
        migrate::{Migration, MigrationType},
    };

    fn migration(version: i64, sql: &'static str) -> Migration {
        Migration::new(
            version,
            format!("migration {version}").into(),
            MigrationType::Simple,
            sql.into_sql_str(),
            false,
        )
    }
    fn request(migrations: &sqlx::migrate::Migrator) -> MigrationRequest {
        MigrationRequest {
            request_id: "test-request".into(),
            bundle_digest: "a".repeat(64),
            expected_ledger_fingerprint: fingerprint(&[]).expect("empty ledger fingerprint"),
            pending: migrations
                .iter()
                .map(|migration| PendingMigration {
                    version: migration.version,
                    checksum: hex(&migration.checksum),
                })
                .collect(),
        }
    }
    #[test]
    fn should_hold_drift_unknown_dirty_and_unreviewed_migrations() {
        let migrations = sqlx::migrate::Migrator::with_migrations(vec![migration(1, "SELECT 1")]);
        let approved = request(&migrations);
        for ledger in [
            vec![LedgerEntry {
                version: 2,
                checksum: "a".repeat(96),
                success: true,
            }],
            vec![LedgerEntry {
                version: 1,
                checksum: "b".repeat(96),
                success: true,
            }],
            vec![LedgerEntry {
                version: 1,
                checksum: approved.pending[0].checksum.clone(),
                success: false,
            }],
        ] {
            assert!(validate_plan(&ledger, &approved, &migrations).is_err());
        }
        let mut nontransactional = migration(2, "SELECT 1");
        nontransactional.no_tx = true;
        let maintenance = sqlx::migrate::Migrator::with_migrations(vec![nontransactional]);
        assert!(matches!(
            validate_plan(&[], &request(&maintenance), &maintenance),
            Err(OperationError::Nontransactional)
        ));
        let mut unreviewed = request(&migrations);
        unreviewed.pending.clear();
        assert!(validate_plan(&[], &unreviewed, &migrations).is_err());
    }
    #[tokio::test]
    async fn should_apply_reviewed_migrations_reconcile_lost_response_and_reject_stale_ledger() {
        let reservation = test_api::reserve_exclusive()
            .await
            .expect("reserve PostgreSQL");
        reservation
            .scope(async {
                let pool = test_api::get_postgres_client().await;
                let mut connection = pool.acquire().await.expect("acquire PostgreSQL");
                sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION pg_database_owner; GRANT USAGE ON SCHEMA public TO PUBLIC")
                    .execute(&mut *connection)
                    .await
                    .expect("reset isolated local database");
                let migrations = sqlx::migrate::Migrator::with_migrations(vec![
                    migration(1, "CREATE TABLE reviewed_first (id bigint)"),
                    migration(2, "CREATE TABLE reviewed_second (id bigint)"),
                ]);
                let applied =
                    migrate_connection(&mut connection, "dev", request(&migrations), &migrations)
                        .await
                        .expect("apply approved migrations");
                assert_eq!(applied.status, "APPLIED");
                assert_eq!(applied.applied.len(), 2);
                let retried =
                    migrate_connection(&mut connection, "dev", request(&migrations), &migrations)
                        .await
                        .expect("reconcile committed response");
                assert_eq!(retried.status, "NO_OP");
                assert_eq!(retried.ledger_fingerprint, applied.ledger_fingerprint);
                sqlx::query(
                    "UPDATE _sqlx_migrations SET checksum = decode('aa','hex') WHERE version=1",
                )
                .execute(&mut *connection)
                .await
                .expect("simulate drift");
                assert!(
                    migrate_connection(&mut connection, "dev", request(&migrations), &migrations)
                        .await
                        .is_err()
                );
            })
            .await;
    }
    #[tokio::test]
    async fn should_keep_committed_prefix_and_bound_lock_wait_after_midway_failure() {
        let reservation = test_api::reserve_exclusive()
            .await
            .expect("reserve PostgreSQL");
        reservation
            .scope(async {
                let pool = test_api::get_postgres_client().await;
                let mut connection = pool.acquire().await.expect("acquire PostgreSQL");
                sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public AUTHORIZATION pg_database_owner; GRANT USAGE ON SCHEMA public TO PUBLIC")
                    .execute(&mut *connection)
                    .await
                    .expect("reset isolated local database");
                let migrations = sqlx::migrate::Migrator::with_migrations(vec![
                    migration(1, "CREATE TABLE committed_first (id bigint)"),
                    migration(2, "SELECT 1/0"),
                ]);
                assert!(
                    migrate_connection(&mut connection, "dev", request(&migrations), &migrations)
                        .await
                        .is_err()
                );
                let actual = ledger(&mut connection)
                    .await
                    .expect("inspect partial outcome");
                assert_eq!(actual.len(), 1);
                assert_eq!(actual[0].version, 1);
                let pending = validate_plan(&actual, &request(&migrations), &migrations)
                    .expect("reconcile approved prefix");
                assert_eq!(pending[0].version, 2);
                let mut contender = pool.acquire().await.expect("contender connection");
                connection.lock().await.expect("own migration lock");
                sqlx::query("SET statement_timeout='50ms'")
                    .execute(&mut *contender)
                    .await
                    .expect("bound contender lock wait");
                assert!(
                    migrate_connection(&mut contender, "dev", request(&migrations), &migrations)
                        .await
                        .is_err()
                );
                connection.unlock().await.expect("release migration lock");
                let mut connection = pool.acquire().await.expect("fresh inspector connection");
                sqlx::raw_sql("BEGIN READ ONLY; SELECT 1")
                    .execute(&mut *connection)
                    .await
                    .expect("read-only snapshot");
                assert!(
                    sqlx::query("CREATE TABLE forbidden_inspection_write (id bigint)")
                        .execute(&mut *connection)
                        .await
                        .is_err()
                );
                sqlx::query("ROLLBACK")
                    .execute(&mut *connection)
                    .await
                    .expect("rollback read-only transaction");
            })
            .await;
    }
}
