use sqlx::AssertSqlSafe;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub mod crawler_domain_configuration_repository;

pub const LOCAL_POSTGRES_HOST: &str = "localhost";
pub const LOCAL_POSTGRES_PORT: u16 = 5432;
pub const LOCAL_POSTGRES_USER: &str = "postgres";
pub const LOCAL_POSTGRES_PASSWORD: &str = "postgres";
pub const LOCAL_POSTGRES_ADMIN_DB: &str = "postgres";

pub const SERVER_DB_NAME: &str = "crawler_server";
pub const DEMO_DB_NAME: &str = "crawler_demo";
pub const DEMO_SCRAPER_DB_NAME: &str = "crawler_demo_scraper";
pub const DEMO_SPIDER_DB_NAME: &str = "crawler_demo_spider";

pub fn database_url(db_name: &str) -> String {
    format!(
        "postgres://{user}:{password}@{host}:{port}/{db}",
        user = LOCAL_POSTGRES_USER,
        password = LOCAL_POSTGRES_PASSWORD,
        host = LOCAL_POSTGRES_HOST,
        port = LOCAL_POSTGRES_PORT,
        db = db_name
    )
}

pub fn server_db_url() -> String {
    database_url(SERVER_DB_NAME)
}

pub fn demo_db_url() -> String {
    database_url(DEMO_DB_NAME)
}

pub fn demo_scraper_db_url() -> String {
    database_url(DEMO_SCRAPER_DB_NAME)
}

pub fn demo_spider_db_url() -> String {
    database_url(DEMO_SPIDER_DB_NAME)
}

fn docker_compose_file() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docker-compose.yml")
        .to_string_lossy()
        .to_string()
}

pub fn start_local_postgres() -> Result<(), String> {
    let compose_file = docker_compose_file();
    let status = Command::new("docker")
        .args(["compose", "-f", &compose_file, "up", "-d"])
        .status()
        .map_err(|e| format!("failed to run docker compose up -d: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "docker compose up -d returned non-zero exit status: {:?}",
            status.code()
        ))
    }
}

async fn connect_admin_pool_with_retry() -> Result<PgPool, String> {
    let admin_url = database_url(LOCAL_POSTGRES_ADMIN_DB);
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(200);

    loop {
        attempt += 1;
        match PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&admin_url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) if attempt < 30 => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
                if attempt >= 29 {
                    return Err(format!("postgres did not become ready in time: {e}"));
                }
            }
            Err(e) => return Err(format!("failed to connect to postgres admin DB: {e}")),
        }
    }
}

async fn create_database_if_missing(pool: &PgPool, db_name: &str) -> Result<(), String> {
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    ))
    .bind(db_name)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("failed checking database '{db_name}' existence: {e}"))?;

    if exists {
        return Ok(());
    }

    // PostgreSQL delimited identifiers escape a quote by doubling it.
    let escaped = db_name.replace('"', "\"\"");
    let create_sql = AssertSqlSafe(format!(r#"CREATE DATABASE "{escaped}""#));
    sqlx::query(create_sql)
        .execute(pool)
        .await
        .map_err(|e| format!("failed creating database '{db_name}': {e}"))?;

    Ok(())
}

pub async fn bootstrap_local_database(db_name: &str) -> Result<(), String> {
    start_local_postgres()?;
    let admin_pool = connect_admin_pool_with_retry().await?;
    create_database_if_missing(&admin_pool, db_name).await
}

pub async fn bootstrap_all_local_databases() -> Result<(), String> {
    start_local_postgres()?;
    let admin_pool = connect_admin_pool_with_retry().await?;
    for db_name in [
        SERVER_DB_NAME,
        DEMO_DB_NAME,
        DEMO_SCRAPER_DB_NAME,
        DEMO_SPIDER_DB_NAME,
    ] {
        create_database_if_missing(&admin_pool, db_name).await?;
    }
    Ok(())
}
