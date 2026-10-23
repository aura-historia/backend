use super::{Kind, Result, TestContext, failure, lease};
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, ConnectOptions, Connection, Executor, PgConnection, PgPool};
use std::path::Path;

struct Baseline {
    key: String,
    scripts: Vec<String>,
    business: bool,
}

impl Baseline {
    fn read(fixture: &crate::Postgres) -> Result<Self> {
        let root = Path::new(env!("CARGO_WORKSPACE_DIR"));
        let directory = root.join(fixture.migrations_dir).canonicalize()?;
        if !directory.starts_with(root.canonicalize()?) || !directory.is_dir() {
            return Err(failure(
                "Postgres baseline must be a workspace migration directory",
            ));
        }
        let mut paths = std::fs::read_dir(&directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|path| path.extension().is_some_and(|extension| extension == "sql"));
        paths.sort();
        if paths.is_empty() {
            return Err(failure("Postgres baseline has no SQL migrations"));
        }
        if let Some(script) = fixture.setup_script {
            let script = root.join(script).canonicalize()?;
            if !script.starts_with(root.canonicalize()?) {
                return Err(failure(
                    "Postgres setup script must belong to the workspace",
                ));
            }
            paths.push(script);
        }
        let mut hash = Sha256::new();
        hash.update(b"aura-postgres-template-v1\0");
        hash.update(fixture.migrations_dir.as_bytes());
        hash.update([u8::from(fixture.setup_script.is_some())]);
        let mut scripts = vec![];
        for path in paths {
            if !path.canonicalize()?.starts_with(root.canonicalize()?) {
                return Err(failure(
                    "Postgres migration file must belong to the workspace",
                ));
            }
            let script = std::fs::read_to_string(&path)?;
            hash.update(path.strip_prefix(root)?.to_string_lossy().as_bytes());
            hash.update((script.len() as u64).to_le_bytes());
            hash.update(script.as_bytes());
            scripts.push(script);
        }
        Ok(Self {
            key: hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            scripts,
            business: fixture.migrations_dir == "migrations",
        })
    }
}

impl TestContext {
    pub(crate) async fn prepare_postgres(&self, fixture: &crate::Postgres) -> Result<()> {
        self.active()?;
        let fixture_copy = *fixture;
        let baseline = tokio::task::spawn_blocking(move || Baseline::read(&fixture_copy)).await??;
        {
            let existing = self.0.baseline.lock().expect("baseline lock");
            if let Some(key) = existing.as_ref() {
                return if key == &baseline.key {
                    Ok(())
                } else {
                    Err(failure(
                        "one context cannot mix different Postgres baselines",
                    ))
                };
            }
        }
        let allocation = lease::Allocation::current().await?;
        let lock = lease::exclusive_file(allocation.directory.join("templates.lock")).await?;
        if let Some(key) = self.0.baseline.lock().expect("baseline lock").as_ref() {
            return if key == &baseline.key {
                Ok(())
            } else {
                Err(failure(
                    "one context cannot mix different Postgres baselines",
                ))
            };
        }
        self.active()?;
        let template = format!("aura_template_{}", &baseline.key[..32]);
        let ready = allocation
            .directory
            .join(format!("template-{}.ready", baseline.key));
        let options = crate::postgres::base_options().await;
        let mut administrator = options.clone().connect().await?;
        let _lock = prepare_template(
            &baseline,
            &template,
            ready,
            &options,
            &mut administrator,
            lock,
        )
        .await?;
        let database = self.own(Kind::Database, self.name("db", 63, "")?)?;
        self.persist_resources().await?;
        // A new generation fences old connections/writers. Recovery drops the previous database first.
        drop_database(&mut administrator, &database).await?;
        administrator
            .execute(AssertSqlSafe(format!(
                "CREATE DATABASE {} TEMPLATE {}",
                quote(&database),
                quote(&template)
            )))
            .await?;
        administrator.close().await?;
        *self.0.postgres.lock().expect("context Postgres lock") = Some(options.database(&database));
        *self.0.baseline.lock().expect("baseline lock") = Some(baseline.key);
        Ok(())
    }

    /// A fresh pool on the current runtime, explicitly registered for teardown.
    pub async fn postgres_pool(&self) -> Result<PgPool> {
        self.active()?;
        let options = self
            .0
            .postgres
            .lock()
            .expect("context Postgres lock")
            .clone()
            .ok_or_else(|| {
                failure("declare and set up a Postgres baseline before acquiring a scoped pool")
            })?;
        let pool = PgPool::connect_with(options).await?;
        self.register_postgres_pool(pool.clone())?;
        Ok(pool)
    }

    pub(crate) async fn close_postgres_pools(&self) {
        let pools = std::mem::take(&mut *self.0.pools.lock().expect("context pool lock"));
        for pool in pools {
            pool.close().await;
        }
    }

    pub fn register_postgres_pool(&self, pool: PgPool) -> Result<()> {
        self.active()?;
        let mut pools = self.0.pools.lock().expect("context pool lock");
        self.active()?;
        pools.push(pool);
        Ok(())
    }

    pub fn postgres_connect_options(&self) -> Result<sqlx::postgres::PgConnectOptions> {
        self.active()?;
        self.0
            .postgres
            .lock()
            .expect("context Postgres lock")
            .clone()
            .ok_or_else(|| failure("Postgres baseline is not initialized"))
    }
}

async fn prepare_template(
    baseline: &Baseline,
    template: &str,
    ready: std::path::PathBuf,
    options: &sqlx::postgres::PgConnectOptions,
    administrator: &mut PgConnection,
    lock: lease::FileLock,
) -> Result<lease::FileLock> {
    let marker_path = ready.clone();
    let (lock, marker) = tokio::task::spawn_blocking(move || -> Result<_> {
        let marker = match std::fs::read_to_string(marker_path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok((lock, marker))
    })
    .await??;
    let guard = if marker.is_none() {
        drop_database(administrator, template).await?;
        administrator
            .execute(AssertSqlSafe(format!(
                "CREATE DATABASE {} TEMPLATE template0",
                quote(template)
            )))
            .await?;
        apply_baseline(baseline, template, options).await?;
        administrator
            .execute(AssertSqlSafe(format!(
                "ALTER DATABASE {} WITH IS_TEMPLATE true ALLOW_CONNECTIONS false",
                quote(template)
            )))
            .await?;
        let key = baseline.key.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            use std::io::Write;
            let temporary = ready.with_extension("ready.tmp");
            let mut marker = lease::lock_file(&temporary)?;
            marker.set_len(0)?;
            marker.write_all(key.as_bytes())?;
            marker.sync_data()?;
            std::fs::rename(temporary, &ready)?;
            // Keep the cross-process template guard until publication has completed,
            // including when the requesting task has been cancelled.
            Ok(lock)
        })
        .await??
    } else {
        if marker.as_deref() != Some(&baseline.key) {
            return Err(failure("Postgres baseline readiness identity is invalid"));
        }
        lock
    };
    Ok(guard)
}

async fn apply_baseline(
    baseline: &Baseline,
    template: &str,
    options: &sqlx::postgres::PgConnectOptions,
) -> Result<()> {
    let mut connection = options.clone().database(template).connect().await?;
    if baseline.business {
        connection.execute(AssertSqlSafe("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'aura_replication') THEN CREATE ROLE aura_replication NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE; END IF; END $$;")).await?;
    }
    for script in &baseline.scripts {
        connection.execute(AssertSqlSafe(script.as_str())).await?;
    }
    connection.close().await?;
    Ok(())
}

pub(super) async fn remove(database: &str) -> Result<()> {
    let mut administrator = crate::postgres::base_options().await.connect().await?;
    drop_database(&mut administrator, database).await?;
    administrator.close().await?;
    Ok(())
}

async fn drop_database(connection: &mut PgConnection, database: &str) -> Result<()> {
    connection
        .execute(AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            quote(database)
        )))
        .await?;
    Ok(())
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod baseline_tests {
    use super::*;

    #[test]
    fn should_key_the_actual_migration_setup_contract_and_reject_invalid_declarations() {
        let fixture = crate::Postgres::new("src/test-api/tests/fixtures/rds_migrations");
        let baseline = Baseline::read(&fixture).unwrap();
        assert_eq!(baseline.key, Baseline::read(&fixture).unwrap().key);
        let seeded = Baseline::read(&crate::Postgres::with_setup_script(
            fixture.migrations_dir,
            "src/test-api/tests/fixtures/postgres_setup.sql",
        ))
        .unwrap();
        assert_ne!(baseline.key, seeded.key);
        assert_eq!(baseline.scripts.len() + 1, seeded.scripts.len());
        for path in [
            "../",
            "src/test-api/src/context",
            "src/test-api/tests/fixtures/missing",
        ] {
            assert!(Baseline::read(&crate::Postgres::new(path)).is_err());
        }
    }
}
