use crate::IntegrationTestService;
use async_trait::async_trait;
use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, Executor, PgConnection, PgPool};
use std::path::Path;
use std::process::{Command, Stdio};
use std::str::FromStr;
use tokio::sync::OnceCell;
use tracing::debug;

const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";
const POSTGRES_PORT: u16 = 5432;
const POSTGRES_CONTAINER_NAME: &str = "aura-historia-aws-backend-postgres-test";

/// Guards the one-time startup of the Postgres container.
///
/// [`tokio::sync::OnceCell`] is used so concurrent async callers all await the same
/// initialisation future instead of racing to start duplicate containers.
static POSTGRES_CONTAINER_STARTED: OnceCell<()> = OnceCell::const_new();

fn connection_string() -> String {
    format!(
        "postgres://{}:{}@localhost:{}/{}",
        POSTGRES_USER, POSTGRES_PASSWORD, POSTGRES_PORT, POSTGRES_DB,
    )
}

/// Opens a fresh [`PgConnection`] to the test Postgres container.
///
/// Each call establishes a new TCP connection. It is not subject to any pool semaphore and
/// is fully owned by the current Tokio runtime. The caller is responsible for dropping it
/// before their runtime shuts down.
async fn open_connection() -> PgConnection {
    let opts = PgConnectOptions::from_str(&connection_string())
        .expect("shouldn't fail parsing Postgres connection string");
    opts.connect()
        .await
        .expect("shouldn't fail connecting to Postgres test container")
}

/// Ensures the Postgres container is running, starting it at most once per process.
///
/// All concurrent callers await the same [`OnceCell`] initialisation future, so only one
/// container is ever started. The container handle is intentionally leaked — it lives for
/// the entire test-suite binary and is cleaned up by the `atexit` handler.
async fn ensure_container_started() {
    POSTGRES_CONTAINER_STARTED
        .get_or_init(|| async {
            install_cleanup();
            // Remove any container left over from a previous aborted run.
            let _ = Command::new("docker")
                .args(["rm", "-f", POSTGRES_CONTAINER_NAME])
                .stderr(Stdio::null())
                .status();

            use testcontainers::ImageExt;
            use testcontainers::core::IntoContainerPort;
            use testcontainers::runners::AsyncRunner;
            use testcontainers_modules::postgres::Postgres as PgImage;

            let container = PgImage::default()
                .with_user(POSTGRES_USER)
                .with_password(POSTGRES_PASSWORD)
                .with_db_name(POSTGRES_DB)
                .with_container_name(POSTGRES_CONTAINER_NAME)
                .with_mapped_port(POSTGRES_PORT, POSTGRES_PORT.tcp())
                .start()
                .await
                .expect("shouldn't fail starting Postgres test container");

            debug!("Successfully started Postgres test container.");

            // Leak the handle intentionally: the container must stay alive for the whole
            // test-suite. The atexit handler takes care of removing it on process exit.
            std::mem::forget(container);
        })
        .await;
}

extern "C" fn cleanup() {
    let _ = Command::new("docker")
        .args(["rm", "-f", POSTGRES_CONTAINER_NAME])
        .status();
}

/// Installs cleanup hooks so that the Postgres container is removed both on normal
/// process exit (`atexit`) and on an interrupted exit (`SIGINT` / `SIGTERM`).
fn install_cleanup() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        unsafe { libc::atexit(cleanup) };
        crate::signal::register_signal_cleanup(|| cleanup());
    });
}

/// Returns a fresh [`PgPool`] connected to the test Postgres container.
///
/// A **new pool is created on every call** and is owned by the caller. This is intentional:
/// `#[tokio::test]` creates a separate Tokio runtime per test and shuts it down when the
/// test ends. `PgPool` internally spawns tasks (e.g. `return_to_pool`) on the current
/// runtime via `tokio::runtime::Handle::spawn`. When the runtime shuts down those tasks are
/// dropped without running, permanently leaking the pool's internal semaphore permits. After
/// only a handful of tests the pool's `acquire` call times out even though no real connections
/// are in use.
///
/// Returning an owned, per-call pool means the caller drops it at the end of the test
/// function, while the test's runtime is still alive, so all pool-internal cleanup futures
/// are properly driven to completion.
///
/// # Returns
///
/// A newly created [`PgPool`] that the caller should drop before its Tokio runtime shuts down
/// (i.e. before the test function returns).
pub async fn get_postgres_client() -> PgPool {
    ensure_container_started().await;

    let pool = PgPool::connect(&connection_string())
        .await
        .expect("shouldn't fail creating Postgres pool for test container");

    debug!("Successfully created Postgres PgPool for current test.");
    pool
}

/// Test helper representing a plain Postgres database for integration tests.
///
/// Unlike AWS service helpers this helper does **not** use LocalStack. It spins up a real
/// Postgres Docker container via [`testcontainers`] and manages it independently.
///
/// # Lifecycle
///
/// - **Before each test** (`set_up`): Starts the Postgres container (once per process), then
///   opens a fresh connection, runs every `*.sql` file found in `migrations_dir` in
///   lexicographic (filename) order — mirroring exactly what `sqlx::migrate!` does at runtime —
///   then runs `setup_script` when supplied.
/// - **After each test** (`tear_down`): Opens a fresh connection and truncates every user
///   table in the `public` schema so that each test starts with a clean slate. Table
///   definitions (DDL) are preserved.
///
/// # Connection strategy
///
/// `set_up` and `tear_down` each open a **new** [`PgConnection`] and close it when done.
/// This avoids cross-runtime resource invalidation: `#[tokio::test]` creates a new Tokio
/// runtime per test and shuts it down afterwards. Any I/O handle (socket, timer, etc.)
/// created on one runtime becomes unusable on another. A fresh connection per call is the
/// only correct approach when the service struct is shared as a `const` across tests.
///
/// # Usage
///
/// ```rust
/// use test_api::*;
///
/// const POSTGRES: Postgres = Postgres::new("migrations");
///
/// #[aura_integration_test(services = [POSTGRES])]
/// async fn should_insert_and_read_row() {
///     let pool = get_postgres_client().await;
///     sqlx::query("INSERT INTO items (id) VALUES (1)").execute(pool).await.unwrap();
/// }
/// ```
///
/// # Notes
///
/// - `service_names` returns `&[]` because Postgres is not a LocalStack service. The
///   `#[aura_integration_test]` macro will still start LocalStack if other services in the same
///   test require it.
/// - The container is shared across all tests within the same test-suite binary. Only the
///   data is reset between tests.
/// - Adding a new migration file to `migrations_dir` is automatically picked up by all tests
///   — no changes to test code required.
#[derive(Debug, Clone, Copy)]
pub struct Postgres {
    /// Path to the directory containing versioned `*.sql` migration files, relative to the
    /// workspace root. Files are executed in lexicographic (filename) order, matching the
    /// ordering used by `sqlx::migrate!` at runtime.
    pub migrations_dir: &'static str,
    /// Optional SQL file, relative to workspace root, run after migrations and before the test.
    pub setup_script: Option<&'static str>,
}

impl Postgres {
    pub const fn new(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
        }
    }

    pub const fn with_setup_script(
        migrations_dir: &'static str,
        setup_script: &'static str,
    ) -> Self {
        Self {
            migrations_dir,
            setup_script: Some(setup_script),
        }
    }
}

#[async_trait]
impl IntegrationTestService for Postgres {
    /// Returns an empty slice because Postgres is managed independently of LocalStack.
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Starts the Postgres container (once), then runs all migrations in `migrations_dir`
    /// in lexicographic order.
    async fn set_up(&self) {
        ensure_container_started().await;

        let workspace_root = env!("CARGO_WORKSPACE_DIR");
        let dir_path = Path::new(workspace_root).join(self.migrations_dir);

        // Collect all *.sql files and sort by filename so they run in migration order.
        let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to read migrations directory '{}': {e}",
                    dir_path.display()
                )
            })
            .filter_map(|entry| {
                let entry = entry.expect("Failed to read directory entry");
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("sql") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect();

        entries.sort();

        let mut conn = open_connection().await;

        for path in &entries {
            let sql = std::fs::read_to_string(path).unwrap_or_else(|e| {
                panic!("Failed to read migration file '{}': {e}", path.display())
            });

            conn.execute(sqlx::raw_sql(&sql)).await.unwrap_or_else(|e| {
                panic!("Failed to execute migration file '{}': {e}", path.display())
            });

            debug!(path = %path.display(), "Applied migration file.");
        }

        if let Some(setup_script) = self.setup_script {
            let script_path = Path::new(workspace_root).join(setup_script);
            let sql = std::fs::read_to_string(&script_path).unwrap_or_else(|e| {
                panic!(
                    "Failed to read setup script '{}': {e}",
                    script_path.display()
                )
            });
            conn.execute(sqlx::raw_sql(&sql)).await.unwrap_or_else(|e| {
                panic!(
                    "Failed to execute setup script '{}': {e}",
                    script_path.display()
                )
            });
            debug!(path = %script_path.display(), "Applied setup script.");
        }

        debug!(
            migrations_dir = self.migrations_dir,
            count = entries.len(),
            "All migration files applied."
        );
    }

    /// Truncates all user tables in the `public` schema to ensure test isolation.
    ///
    /// Table definitions (DDL) are intentionally kept intact so that the next test's
    /// `set_up` can rely on `CREATE TABLE IF NOT EXISTS` being a no-op.
    async fn tear_down(&self) {
        let mut conn = open_connection().await;

        // Collect all user table names from the public schema.
        let tables: Vec<String> = sqlx::query_scalar::<_, String>(
            "SELECT table_name \
             FROM information_schema.tables \
             WHERE table_schema = 'public' \
               AND table_type = 'BASE TABLE'",
        )
        .fetch_all(&mut conn)
        .await
        .expect("shouldn't fail querying table names for tear-down");

        if tables.is_empty() {
            return;
        }

        // Build a single TRUNCATE statement for all tables. CASCADE handles FK constraints.
        let table_list = tables
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ");

        let truncate_sql = format!("TRUNCATE TABLE {table_list} RESTART IDENTITY CASCADE");

        conn.execute(sqlx::raw_sql(&truncate_sql))
            .await
            .expect("shouldn't fail truncating tables for tear-down");

        debug!(
            tables = ?tables,
            "Truncated all public tables for test isolation."
        );
    }
}
