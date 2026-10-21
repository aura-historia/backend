mod topology;

#[cfg(all(test, feature = "sqs", feature = "ses", feature = "s3"))]
mod runtime_tests;

use self::topology::AwsEmulatorTopology;
use aws_config::{BehaviorVersion, SdkConfig};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use testcontainers::core::{IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;
use tracing::debug;

const CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-floci-test";
const IMAGE_TAG: &str = "2.3.0";
pub const AWS_EMULATOR_CONTAINER_PORT: u16 = 4566;

pub(crate) fn aws_emulator_container_name() -> String {
    format!("{CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

static CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();
static ENDPOINT_URL: OnceLock<String> = OnceLock::new();

/// Returns the process fixture's host endpoint. Start it with [`get_aws_emulator`] first.
pub fn get_endpoint_url() -> &'static str {
    if let Some(config) = crate::environment::managed_environment() {
        return config
            .aws_endpoint()
            .expect("managed profile has no AWS emulator");
    }
    ENDPOINT_URL
        .get()
        .expect("AWS emulator endpoint not initialized; call get_aws_emulator first")
}

/// Uses fixed test credentials and region; never loads live AWS credentials.
///
/// The shared configuration leaves the HTTP client unset. Each service client constructs
/// its own connector, so pooled I/O is owned by that client and its runtime.
pub async fn get_aws_config() -> &'static SdkConfig {
    CONFIG
        .get_or_init(|| async {
            aws_config::defaults(BehaviorVersion::latest())
                .credentials_provider(aws_sdk_account::config::Credentials::for_tests())
                .region("eu-central-1")
                .endpoint_url(get_endpoint_url())
                .load()
                .await
        })
        .await
}

/// An attached service capability; managed attachments own no Docker resource.
pub struct AwsEmulatorFixture {
    _container: Option<ContainerAsync<GenericImage>>,
    topology: AwsEmulatorTopology,
}

static AWS_EMULATOR: OnceCell<AwsEmulatorFixture> = OnceCell::const_new();

/// Shares one isolated Floci container per test process. Later requests must fit its topology.
pub async fn get_aws_emulator(
    services: &[&str],
    extra_env_vars: &[(&str, &str)],
) -> &'static AwsEmulatorFixture {
    if let Some(config) = crate::environment::managed_environment() {
        config.assert_aws_topology(services, extra_env_vars);
    }
    let requested_topology = AwsEmulatorTopology::new(services, extra_env_vars);
    let fixture = AWS_EMULATOR
        .get_or_init(|| async {
            if let Some(config) = crate::environment::managed_environment() {
                config.assert_aws_topology(services, extra_env_vars);
                return AwsEmulatorFixture {
                    _container: None,
                    topology: AwsEmulatorTopology::new(
                        &config
                            .aws_services
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        &[],
                    ),
                };
            }
            let normalized_env = requested_topology
                .env
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let (container, port) = spin_up_aws_emulator(&normalized_env).await;
            ENDPOINT_URL
                .set(format!("http://localhost:{port}"))
                .expect("shouldn't fail setting AWS emulator endpoint URL");
            debug!(services = ?requested_topology.services, "Floci process fixture started");
            AwsEmulatorFixture {
                _container: Some(container),
                topology: requested_topology,
            }
        })
        .await;
    AwsEmulatorTopology::new(services, extra_env_vars).assert_compatible_with(&fixture.topology);
    fixture
}

fn docker_remove(name: &str) {
    let _ = Command::new("docker")
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

extern "C" fn cleanup() {
    let name = aws_emulator_container_name();
    docker_remove(&name);

    // Force-removing the emulator skips its shutdown hooks. Reap only this process's children.
    let namespace_filter = format!("label=floci_namespace={name}");
    if let Ok(out) = Command::new("docker")
        .args(["ps", "-aq", "--filter", &namespace_filter])
        .output()
    {
        for id in String::from_utf8_lossy(&out.stdout).lines() {
            docker_remove(id);
        }
    }
    if let Ok(out) = Command::new("docker")
        .args(["volume", "ls", "-q", "--filter", &namespace_filter])
        .output()
    {
        for id in String::from_utf8_lossy(&out.stdout).lines() {
            let _ = Command::new("docker")
                .args(["volume", "rm", id])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn install_cleanup() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        unsafe { libc::atexit(cleanup) };
        crate::signal::register_signal_cleanup(|| cleanup());
    });
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("shouldn't fail binding to a random port")
        .local_addr()
        .expect("shouldn't fail reading local address")
        .port()
}

/// Starts Floci with its real service implementations and a random host port.
/// Docker-backed resources are namespaced by PID and removed on exit or interruption.
pub async fn spin_up_aws_emulator(
    extra_env_vars: &[(&str, &str)],
) -> (ContainerAsync<GenericImage>, u16) {
    assert!(
        crate::environment::managed_environment().is_none(),
        "managed test processes cannot start Floci containers"
    );
    let started = std::time::Instant::now();
    install_cleanup();
    let _ = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .try_init();

    let port = find_free_port();
    let request = GenericImage::new("floci/floci", IMAGE_TAG)
        .with_exposed_port(AWS_EMULATOR_CONTAINER_PORT.tcp())
        .with_wait_for(WaitFor::healthcheck())
        .with_container_name(aws_emulator_container_name())
        .with_env_var("FLOCI_STORAGE_MODE", "memory")
        .with_env_var("FLOCI_DEFAULT_REGION", "eu-central-1")
        .with_env_var("FLOCI_BASE_URL", format!("http://localhost:{port}"))
        .with_env_var("FLOCI_SERVICES_OPENSEARCH_MOCK", "false");
    let request = extra_env_vars
        .iter()
        .fold(request, |request, (key, value)| {
            request.with_env_var(*key, *value)
        });
    let container = request
        .with_env_var(
            "FLOCI_DOCKER_RESOURCE_NAMESPACE",
            aws_emulator_container_name(),
        )
        .with_mount(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ))
        .with_mapped_port(port, AWS_EMULATOR_CONTAINER_PORT.tcp())
        .start()
        .await
        .expect("shouldn't fail starting Floci");
    crate::benchmark::record_timing("FLOCI_STARTUP", "PROCESS", started.elapsed());
    (container, port)
}
