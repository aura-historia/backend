use crate::{
    IntegrationTestService, get_postgres_client, get_postgres_host_gateway_connection_string,
    postgres::get_postgres_host_port,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use sqlx::{AssertSqlSafe, Executor};
use std::net::{SocketAddr, TcpListener};
use std::sync::OnceLock;
use std::time::Duration;
use testcontainers::core::{Host, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;
use tracing::debug;

const REDIS_CONTAINER_PORT: u16 = 6379;
const SEQUIN_CONTAINER_PORT: u16 = 7376;
const SEQUIN_HEALTH_MAX_ATTEMPTS: u8 = 90;
const SEQUIN_STATE_DB_PREFIX: &str = "sequin";
const SECRET_KEY_BASE: &str = "wDPLYus0pvD6qJhKJICO4vYl782Zjtpew5qRBDp7CZvbWtQmY0eB13If01234567";
const VAULT_KEY: &str = "2Sig69bIpuSm2kv0VQfDekET2qy8qUZGI8v3/h3ASiY=";
const WORKER_WEBHOOK_TABLES: &[&str] = &[
    "public.product_events",
    "public.search_filters",
    "public.search_filter_matches",
];
const NOTIFICATION_DELIVERY_TABLE: &str = "public.notification_deliveries";

static WORKER_WEBHOOK_SEQUIN: OnceCell<RunningSequin> = OnceCell::const_new();
static WORKER_WEBHOOK_PORT: OnceLock<u16> = OnceLock::new();

/// Process-lived Sequin fixture that delivers the worker's CDC tables to its local webhook.
#[derive(Debug, Clone, Copy)]
pub struct Sequin;

impl Sequin {
    pub const fn worker_webhook() -> Self {
        Self
    }
}

#[async_trait]
impl IntegrationTestService for Sequin {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        get_or_start_worker_webhook_sequin().await;
    }
}

#[derive(Debug)]
struct RunningSequin {
    _redis: ContainerAsync<GenericImage>,
    _sequin: ContainerAsync<GenericImage>,
}

/// Returns the fixed process-local address that the worker must bind before source writes.
pub fn get_sequin_worker_webhook_bind_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], worker_webhook_port()))
}

async fn get_or_start_worker_webhook_sequin() -> &'static RunningSequin {
    WORKER_WEBHOOK_SEQUIN
        .get_or_init(|| async {
            let webhook_url = format!(
                "http://host.docker.internal:{}/cdc/sequin",
                worker_webhook_port()
            );
            start_worker_webhook_sequin(&webhook_url).await
        })
        .await
}

async fn start_worker_webhook_sequin(webhook_url: &str) -> RunningSequin {
    let suffix = std::process::id().to_string();
    let state_database = format!("{SEQUIN_STATE_DB_PREFIX}_{suffix}");
    ensure_sequin_state_database(&state_database).await;

    let redis_port = find_free_port();
    let redis = GenericImage::new("redis", "7-alpine")
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .with_mapped_port(redis_port, REDIS_CONTAINER_PORT.tcp())
        .start()
        .await
        .expect("shouldn't fail starting Redis test container for Sequin");

    let config_yaml = sequin_config_yaml(webhook_url, &suffix);
    let config_yaml_base64 = STANDARD.encode(config_yaml);
    let redis_url = format!("redis://host.docker.internal:{redis_port}");
    let sequin_state_pg_url = get_postgres_host_gateway_connection_string(&state_database);

    let sequin_port = find_free_port();
    let sequin = GenericImage::new("sequin/sequin", "latest")
        .with_wait_for(WaitFor::seconds(5))
        .with_env_var("SERVER_PORT", SEQUIN_CONTAINER_PORT.to_string())
        .with_env_var("PG_URL", sequin_state_pg_url)
        .with_env_var("PG_POOL_SIZE", "3")
        .with_env_var("REDIS_URL", redis_url)
        .with_env_var("SECRET_KEY_BASE", SECRET_KEY_BASE)
        .with_env_var("VAULT_KEY", VAULT_KEY)
        .with_env_var("CONFIG_FILE_YAML", config_yaml_base64)
        .with_env_var("TELEMETRY_ENABLED", "false")
        .with_env_var("CRASH_REPORTING_DISABLED", "true")
        .with_host("host.docker.internal", Host::HostGateway)
        .with_mapped_port(sequin_port, SEQUIN_CONTAINER_PORT.tcp())
        .start()
        .await
        .expect("shouldn't fail starting Sequin test container");
    let endpoint_url = format!("http://localhost:{sequin_port}");

    wait_for_sequin_health(&endpoint_url, &sequin).await;
    debug!(%endpoint_url, "Successfully started process-lived Sequin test container.");

    RunningSequin {
        _redis: redis,
        _sequin: sequin,
    }
}

async fn ensure_sequin_state_database(database: &str) {
    let pool = get_postgres_client().await;
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    ))
    .bind(database)
    .fetch_one(&pool)
    .await
    .expect("shouldn't fail checking Sequin state database");

    if !exists {
        pool.execute(AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .await
            .expect("shouldn't fail creating Sequin state database");
    }
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("shouldn't fail binding to a random port")
        .local_addr()
        .expect("shouldn't fail reading local address")
        .port()
}

fn worker_webhook_port() -> u16 {
    *WORKER_WEBHOOK_PORT.get_or_init(find_free_port)
}

fn sequin_config_yaml(webhook_url: &str, suffix: &str) -> String {
    let publication_tables = WORKER_WEBHOOK_TABLES
        .iter()
        .copied()
        .chain(std::iter::once(NOTIFICATION_DELIVERY_TABLE))
        .collect::<Vec<_>>()
        .join(", ");
    let include_tables = WORKER_WEBHOOK_TABLES
        .iter()
        .map(|table| format!("\"{table}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut config = include_str!("sequin/base.yaml")
        .replace("__SUFFIX__", suffix)
        .replace("__POSTGRES_PORT__", &get_postgres_host_port().to_string())
        .replace("__PUBLICATION_TABLES__", &publication_tables);

    let sink_yaml = include_str!("sequin/webhook-sink.yaml")
        .replace("__SUFFIX__", suffix)
        .replace("__WEBHOOK_URL__", webhook_url)
        .replace("__INCLUDE_TABLES__", &include_tables);
    config.push_str(&sink_yaml);

    let notification_delivery_sink_yaml =
        include_str!("sequin/notification-delivery-webhook-sink.yaml")
            .replace("__SUFFIX__", suffix)
            .replace("__WEBHOOK_URL__", webhook_url);
    config.push_str(&format!(
        "  {}",
        notification_delivery_sink_yaml.replace('\n', "\n  ")
    ));

    config
}

async fn wait_for_sequin_health(endpoint_url: &str, container: &ContainerAsync<GenericImage>) {
    let client = reqwest::Client::new();
    let health_url = format!("{endpoint_url}/health");

    for _ in 0..SEQUIN_HEALTH_MAX_ATTEMPTS {
        if let Ok(response) = client.get(&health_url).send().await
            && response.status() == StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let stdout = container.stdout_to_vec().await.unwrap_or_default();
    let stderr = container.stderr_to_vec().await.unwrap_or_default();
    panic!(
        "Sequin health endpoint did not become ready at {health_url}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
}
