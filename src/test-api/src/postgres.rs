use crate::IntegrationTestService;
use async_trait::async_trait;
use sqlx::postgres::PgConnectOptions;
use sqlx::{AssertSqlSafe, ConnectOptions, Executor, PgConnection, PgPool};
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::OnceCell;
use tracing::debug;

const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";
const POSTGRES_CONTAINER_PORT: u16 = 5432;
const POSTGRES_CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-postgres-test";
const POSTGRES_PG_TTL_IMAGE_TAG: &str = "16-pg-ttl-index-v3.0.0";
const POSTGRES_PG_TTL_DOCKERFILE: &str = "src/test-api/postgres/Dockerfile";
const POSTGRES_PG_TTL_CONTEXT: &str = "src/test-api/postgres";
const HOST_GATEWAY: &str = "host.docker.internal";

type MigrationInitializers = Mutex<HashMap<&'static str, Arc<OnceCell<()>>>>;

/// Guards the one-time startup of the Postgres container.
///
/// [`tokio::sync::OnceCell`] is used so concurrent async callers all await the same
/// initialisation future instead of racing to start duplicate containers.
static POSTGRES_CONTAINER_STARTED: OnceCell<()> = OnceCell::const_new();
static POSTGRES_HOST_PORT: OnceLock<u16> = OnceLock::new();
static MIGRATIONS_APPLIED: OnceLock<MigrationInitializers> = OnceLock::new();

fn postgres_container_name() -> String {
    format!("{POSTGRES_CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

fn postgres_host_port() -> u16 {
    *POSTGRES_HOST_PORT
        .get()
        .expect("Postgres host port not initialized; call `ensure_container_started()` first")
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("shouldn't fail binding to a random port")
        .local_addr()
        .expect("shouldn't fail reading local address")
        .port()
}

fn connection_string() -> String {
    postgres_connection_string("localhost", POSTGRES_DB)
}

pub fn get_postgres_host_gateway_connection_string(database: &str) -> String {
    postgres_connection_string(HOST_GATEWAY, database)
}

#[cfg(feature = "sequin")]
pub(crate) fn get_postgres_host_port() -> u16 {
    postgres_host_port()
}

fn postgres_connection_string(host: &str, database: &str) -> String {
    format!(
        "postgres://{}:{}@{}:{}/{}",
        POSTGRES_USER,
        POSTGRES_PASSWORD,
        host,
        postgres_host_port(),
        database,
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
            let name = postgres_container_name();
            let port = find_free_port();
            POSTGRES_HOST_PORT
                .set(port)
                .expect("shouldn't fail setting Postgres host port");

            // Remove any container left over from a previous aborted run of this process id.
            let _ = docker_remove(&name);
            ensure_pg_ttl_image();

            use testcontainers::ImageExt;
            use testcontainers::core::IntoContainerPort;
            use testcontainers::runners::AsyncRunner;
            use testcontainers_modules::postgres::Postgres as PgImage;

            let container = PgImage::default()
                .with_user(POSTGRES_USER)
                .with_password(POSTGRES_PASSWORD)
                .with_db_name(POSTGRES_DB)
                .with_tag(POSTGRES_PG_TTL_IMAGE_TAG)
                .with_cmd([
                    "-c",
                    "fsync=off",
                    "-c",
                    "wal_level=logical",
                    "-c",
                    "shared_preload_libraries=pg_ttl_index",
                ])
                .with_container_name(name)
                .with_mapped_port(port, POSTGRES_CONTAINER_PORT.tcp())
                .start()
                .await
                .expect("shouldn't fail starting Postgres test container");

            let mut connection = open_connection().await;
            connection
                .execute(AssertSqlSafe("CREATE EXTENSION pg_ttl_index"))
                .await
                .expect("should create pg_ttl_index extension in test database");
            connection
                .execute(AssertSqlSafe("SELECT ttl_start_worker()"))
                .await
                .expect("should start pg_ttl_index worker in test database");

            debug!("Successfully started Postgres test container with pg_ttl_index.");

            // Leak the handle intentionally: the container must stay alive for the whole
            // test-suite. The atexit handler takes care of removing it on process exit.
            std::mem::forget(container);
        })
        .await;
}

fn ensure_pg_ttl_image() {
    static IMAGE_BUILT: OnceLock<()> = OnceLock::new();
    IMAGE_BUILT.get_or_init(|| {
        let workspace_root = env!("CARGO_WORKSPACE_DIR");
        let image = format!("postgres:{POSTGRES_PG_TTL_IMAGE_TAG}");
        let status = Command::new("docker")
            .current_dir(workspace_root)
            .args(["build", "--file", POSTGRES_PG_TTL_DOCKERFILE, "--tag"])
            .arg(image)
            .arg(POSTGRES_PG_TTL_CONTEXT)
            .status()
            .unwrap_or_else(|error| panic!("failed to build pg-ttl test image: {error}"));
        assert!(status.success(), "failed to build pinned pg-ttl test image");
    });
}

fn docker_remove(name: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("docker")
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}

extern "C" fn cleanup() {
    let name = postgres_container_name();
    let _ = docker_remove(&name);
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
/// - **Before each test** (`set_up`): Starts the Postgres container once per process.
///   [`Postgres::new`] and [`Postgres::new_schema_once`] apply schema-only migrations once per
///   test process; `setup_script` still runs before each test. [`Postgres::new_per_test`]
///   replays migrations before each test when they provide seed data.
/// - **After each test** (`tear_down`): Opens a fresh connection and truncates application-owned
///   tables in the `public` schema so that each test starts with a clean slate. Extension-owned
///   metadata and table definitions (DDL) are preserved.
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
    migrate_once: bool,
}

async fn apply_migrations(migrations_dir: &'static str) {
    let workspace_root = env!("CARGO_WORKSPACE_DIR");
    let dir_path = Path::new(workspace_root).join(migrations_dir);
    let mut entries: Vec<_> = std::fs::read_dir(&dir_path)
        .unwrap_or_else(|error| {
            panic!(
                "failed to read migrations directory '{}': {error}",
                dir_path.display()
            )
        })
        .filter_map(|entry| {
            let entry =
                entry.unwrap_or_else(|error| panic!("failed to read migration entry: {error}"));
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("sql"))
                .then_some(path)
        })
        .collect();
    entries.sort();

    let mut connection = open_connection().await;
    for path in &entries {
        let sql = std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!(
                "failed to read migration file '{}': {error}",
                path.display()
            )
        });
        connection
            .execute(AssertSqlSafe(sql))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "failed to execute migration file '{}': {error}",
                    path.display()
                )
            });
    }

    debug!(
        migrations_dir,
        count = entries.len(),
        "Applied Postgres migrations."
    );
}

async fn apply_setup_script(setup_script: &'static str) {
    let script_path = Path::new(env!("CARGO_WORKSPACE_DIR")).join(setup_script);
    let sql = std::fs::read_to_string(&script_path).unwrap_or_else(|error| {
        panic!(
            "failed to read setup script '{}': {error}",
            script_path.display()
        )
    });
    let mut connection = open_connection().await;
    connection
        .execute(AssertSqlSafe(sql))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "failed to execute setup script '{}': {error}",
                script_path.display()
            )
        });
    debug!(path = %script_path.display(), "Applied Postgres setup script.");
}

impl Postgres {
    /// Uses a migration directory containing schema only, without migration-provided seed data.
    /// The directory is applied once per test process; data isolation still uses truncation.
    pub const fn new(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: true,
        }
    }

    /// Alias for [`Postgres::new`] for callers that want to document schema-only intent.
    pub const fn new_schema_once(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: true,
        }
    }

    /// Reapplies migrations before each test to restore migration-provided seed data.
    pub const fn new_per_test(migrations_dir: &'static str) -> Self {
        Self {
            migrations_dir,
            setup_script: None,
            migrate_once: false,
        }
    }

    pub const fn with_setup_script(
        migrations_dir: &'static str,
        setup_script: &'static str,
    ) -> Self {
        Self {
            migrations_dir,
            setup_script: Some(setup_script),
            migrate_once: false,
        }
    }
}

#[async_trait]
impl IntegrationTestService for Postgres {
    /// Returns an empty slice because Postgres is managed independently of LocalStack.
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Starts the Postgres container and applies migrations according to the selected lifecycle.
    async fn set_up(&self) {
        ensure_container_started().await;

        if self.migrate_once {
            let migrations = MIGRATIONS_APPLIED
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap_or_else(|error| panic!("Postgres migration state lock poisoned: {error}"))
                .entry(self.migrations_dir)
                .or_insert_with(|| Arc::new(OnceCell::const_new()))
                .clone();
            let migrations_dir = self.migrations_dir;

            migrations
                .get_or_init(|| async move {
                    apply_migrations(migrations_dir).await;
                })
                .await;
        } else {
            apply_migrations(self.migrations_dir).await;
        }

        if let Some(setup_script) = self.setup_script {
            apply_setup_script(setup_script).await;
        }
    }

    /// Truncates application-owned tables in the `public` schema to ensure test isolation.
    ///
    /// Table definitions (DDL) are intentionally kept intact so that the next test's
    /// `set_up` can rely on `CREATE TABLE IF NOT EXISTS` being a no-op.
    async fn tear_down(&self) {
        let mut conn = open_connection().await;

        // Exclude relations owned by installed extensions. Extension metadata must survive
        // per-test cleanup, and this catalog query avoids coupling to extension table names.
        let tables: Vec<String> = sqlx::query_scalar::<_, String>(AssertSqlSafe(
            "SELECT relation.relname \
             FROM pg_class AS relation \
             JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace \
             LEFT JOIN pg_depend AS extension_dependency \
               ON extension_dependency.classid = 'pg_class'::regclass \
              AND extension_dependency.objid = relation.oid \
              AND extension_dependency.deptype = 'e' \
             WHERE namespace.nspname = 'public' \
               AND relation.relkind = 'r' \
               AND extension_dependency.objid IS NULL",
        ))
        .fetch_all(&mut conn)
        .await
        .expect("shouldn't fail querying table names for tear-down");

        if tables.is_empty() {
            return;
        }

        // Build a single TRUNCATE statement for all tables. CASCADE handles FK constraints.
        let table_list = tables
            .iter()
            .map(|table| format!("\"{}\"", table.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(", ");

        let truncate_sql = AssertSqlSafe(format!(
            "TRUNCATE TABLE {table_list} RESTART IDENTITY CASCADE"
        ));

        conn.execute(truncate_sql)
            .await
            .expect("shouldn't fail truncating tables for tear-down");

        debug!(
            tables = ?tables,
            "Truncated application-owned public tables for test isolation."
        );
    }
}
