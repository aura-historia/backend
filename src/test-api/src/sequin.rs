use crate::{
    IntegrationTestService, get_postgres_client, get_postgres_host_gateway_connection_string,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use sqlx::Executor;
use std::net::TcpListener;
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

#[derive(Debug, Clone, Copy)]
pub struct Sequin;

impl Sequin {
    pub const fn new() -> Self {
        Self
    }
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
        get_or_start_sequin().await;
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
        .get_or_init(|| async { start_sequin_without_sink().await })
        .await
}

async fn start_sequin_without_sink() -> RunningSequin {
    start_sequin_container(None).await
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

fn sequin_resource_suffix() -> String {
    let sequence = SEQUIN_SEQUENCE.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", std::process::id(), sequence)
}

fn sequin_config_yaml(webhook_url: Option<&str>, suffix: &str) -> String {
    let sink_yaml = webhook_url
        .map(|url| {
            format!(
                r#"
http_endpoints:
  - name: "worker"
    url: "{url}"

sinks:
  - name: "worker-product-events"
    database: "aura-historia-business-{suffix}"
    source:
      include_tables:
        - "public.product_events"
    actions:
      - insert
    batch_size: 1
    destination:
      type: "webhook"
      http_endpoint: "worker"
      batch: false
"#,
            )
        })
        .unwrap_or_default();

    format!(
        r#"account:
  name: "aura-historia-test"

users:
  - email: "admin@example.com"
    password: "sequinpassword!"

api_tokens:
  - name: "test"
    token: "test-token"

databases:
  - name: "aura-historia-business-{suffix}"
    username: "postgres"
    password: "postgres"
    hostname: "host.docker.internal"
    port: 5432
    database: "postgres"
    pool_size: 3
    slot:
      name: "aura_historia_test_slot_{suffix}"
      create_if_not_exists: true
    publication:
      name: "aura_historia_test_pub_{suffix}"
      create_if_not_exists: true
      init_sql: |-
        create publication aura_historia_test_pub_{suffix} for table public.product_events with (publish_via_partition_root = true)
    await_database:
      timeout_ms: 30000
      interval_ms: 1000

{sink_yaml}"#,
    )
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
