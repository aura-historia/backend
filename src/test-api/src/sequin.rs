use crate::{
    IntegrationTestService, get_postgres_client, get_postgres_host_gateway_connection_string,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use sqlx::Executor;
use std::net::{SocketAddr, TcpListener};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use testcontainers::core::{Host, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;
use tracing::debug;

const REDIS_CONTAINER_PORT: u16 = 6379;
const SEQUIN_CONTAINER_PORT: u16 = 7376;
const SEQUIN_STATE_DB: &str = "sequin";
const SECRET_KEY_BASE: &str = "wDPLYus0pvD6qJhKJICO4dauYPXfO/Yl782Zjtpew5qRBDp7CZvbWtQmY0eB13If";
const VAULT_KEY: &str = "2Sig69bIpuSm2kv0VQfDekET2qy8qUZGI8v3/h3ASiY=";
static SEQUIN_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static DEFAULT_SEQUIN: OnceCell<RunningSequin> = OnceCell::const_new();
static WORKER_WEBHOOK_SEQUIN: OnceCell<RunningSequin> = OnceCell::const_new();
static WORKER_WEBHOOK_PORT: OnceLock<u16> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub struct Sequin {
    mode: SequinMode,
}

impl Sequin {
    pub const fn new() -> Self {
        Self {
            mode: SequinMode::NoSink,
        }
    }

    pub const fn worker_webhook() -> Self {
        Self {
            mode: SequinMode::WorkerWebhook,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SequinMode {
    NoSink,
    WorkerWebhook,
}

impl Default for Sequin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl IntegrationTestService for Sequin {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        match self.mode {
            SequinMode::NoSink => {
                get_or_start_sequin().await;
            }
            SequinMode::WorkerWebhook => {
                get_or_start_worker_webhook_sequin().await;
            }
        }
    }
}

#[derive(Debug)]
pub struct RunningSequin {
    endpoint_url: String,
    _redis: ContainerAsync<GenericImage>,
    _sequin: ContainerAsync<GenericImage>,
}

impl RunningSequin {
    pub fn endpoint_url(&self) -> &str {
        &self.endpoint_url
    }

    pub async fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self._sequin.stdout_to_vec().await.unwrap_or_default())
            .into_owned()
    }

    pub async fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self._sequin.stderr_to_vec().await.unwrap_or_default())
            .into_owned()
    }
}

pub async fn get_or_start_sequin() -> &'static RunningSequin {
    DEFAULT_SEQUIN
        .get_or_init(|| async { start_sequin_container(None).await })
        .await
}

pub async fn get_or_start_worker_webhook_sequin() -> &'static RunningSequin {
    WORKER_WEBHOOK_SEQUIN
        .get_or_init(|| async {
            let port = worker_webhook_port();
            let webhook_url = format!("http://host.docker.internal:{port}/cdc/sequin");
            start_sequin_container(Some(&webhook_url)).await
        })
        .await
}

pub fn get_sequin_worker_webhook_bind_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], worker_webhook_port()))
}

pub async fn start_sequin(webhook_url: &str) -> RunningSequin {
    start_sequin_container(Some(webhook_url)).await
}

async fn start_sequin_container(webhook_url: Option<&str>) -> RunningSequin {
    ensure_sequin_state_database().await;

    let suffix = sequin_resource_suffix();
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
    let sequin_state_pg_url = get_postgres_host_gateway_connection_string(SEQUIN_STATE_DB);

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
    debug!(%endpoint_url, "Successfully started Sequin test container.");

    RunningSequin {
        endpoint_url,
        _redis: redis,
        _sequin: sequin,
    }
}

async fn ensure_sequin_state_database() {
    let pool = get_postgres_client().await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(SEQUIN_STATE_DB)
            .fetch_one(&pool)
            .await
            .expect("shouldn't fail checking Sequin state database");

    if !exists {
        pool.execute(sqlx::raw_sql(&format!("CREATE DATABASE {SEQUIN_STATE_DB}")))
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

fn sequin_resource_suffix() -> String {
    let sequence = SEQUIN_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", std::process::id(), sequence)
}

fn sequin_config_yaml(webhook_url: Option<&str>, suffix: &str) -> String {
    let mut config = include_str!("sequin/base.yaml").replace("__SUFFIX__", suffix);

    if let Some(url) = webhook_url {
        let sink_yaml = include_str!("sequin/webhook-sink.yaml")
            .replace("__SUFFIX__", suffix)
            .replace("__WEBHOOK_URL__", url);
        config.push_str(&sink_yaml);
    }

    config
}

async fn wait_for_sequin_health(endpoint_url: &str, container: &ContainerAsync<GenericImage>) {
    let client = reqwest::Client::new();
    let health_url = format!("{endpoint_url}/health");

    for _ in 0..30 {
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
