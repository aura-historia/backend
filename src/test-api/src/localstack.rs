use aws_config::{BehaviorVersion, SdkConfig};
use std::collections::HashMap;
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use testcontainers::core::{IntoContainerPort, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::localstack::LocalStackPro;
use tokio::sync::OnceCell;
use tracing::{debug, error};

const LOCALSTACK_CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-localstack-test";

/// The fixed TCP port that LocalStack listens on **inside** the container.
///
/// The Docker host maps a random free port to this internal port. Use [`get_endpoint_url()`]
/// to obtain the host-side URL. Use this constant only when constructing URLs that
/// LocalStack itself will resolve from inside the container (e.g., domain custom endpoints).
pub const LOCALSTACK_CONTAINER_PORT: u16 = 4566;

/// Returns a unique container name for this test process, derived from the process ID.
///
/// Using the PID ensures that concurrent test processes on the same machine each
/// manage their own LocalStack container without interfering with one another.
fn localstack_container_name() -> String {
    format!("{LOCALSTACK_CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

/// A lazily-initialized, globally accessible AWS SDK configuration for integration tests.
///
/// This static `OnceCell` holds the result of `aws_config::load()` with LocalStack-specific
/// overrides (e.g., test credentials, custom endpoint, region).
///
/// Initialized once on first use via [`get_aws_config()`].
static CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();

/// Stores the LocalStack endpoint URL (e.g. `http://localhost:54321`) once the container
/// has started and its host-mapped port is known.
///
/// Set by [`get_localstack()`] during container startup. Must be initialized before
/// [`get_aws_config()`] is called.
static ENDPOINT_URL: OnceLock<String> = OnceLock::new();

/// Returns the LocalStack endpoint URL (e.g. `http://localhost:54321`).
///
/// # Panics
///
/// Panics if called before [`get_localstack()`] has started the container.
pub fn get_endpoint_url() -> &'static str {
    ENDPOINT_URL
        .get()
        .expect("LocalStack endpoint URL not yet initialized; call `get_localstack()` first")
}

/// Loads and returns a static reference to the AWS SDK configuration for LocalStack.
///
/// This function ensures that the configuration is loaded only once using `OnceCell`.
/// It configures the AWS SDK to use:
/// - Test credentials (`Credentials::for_tests()`)
/// - Static region (`"eu-central-1"`)
/// - LocalStack endpoint at [Endpoint-URL](get_endpoint_url)
///
/// # Returns
///
/// A reference to a globally-initialized `SdkConfig` instance suitable for use with AWS clients.
pub async fn get_aws_config() -> &'static SdkConfig {
    let cfg = CONFIG
        .get_or_init(|| async {
            aws_config::defaults(BehaviorVersion::latest())
                .credentials_provider(aws_sdk_account::config::Credentials::for_tests())
                .region("eu-central-1")
                .endpoint_url(get_endpoint_url())
                .load()
                .await
        })
        .await;
    debug!("Successfully set up AWS-Config.");
    cfg
}

static LOCALSTACK: OnceCell<ContainerAsync<LocalStackPro>> = OnceCell::const_new();

pub async fn get_localstack(
    services: &[&str],
    extra_env_vars: &[(&str, &str)],
) -> &'static ContainerAsync<LocalStackPro> {
    LOCALSTACK
        .get_or_init(|| async {
            install_cleanup();
            // Spins up with the first (!) supplied services only.
            // No dealbreaker for now as each test-suite has it's own OnceCell
            // And all tests within a test-suite require the same services
            let (container, port) =
                spin_up_localstack_with_services(services, extra_env_vars).await;
            ENDPOINT_URL
                .set(format!("http://localhost:{port}"))
                .expect("shouldn't fail setting LocalStack endpoint URL");
            container
        })
        .await
}

fn docker_remove(name: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("docker")
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}

extern "C" fn cleanup() {
    let name = localstack_container_name();
    let _ = docker_remove(&name);

    // Remove ephemeral containers spawned by LocalStack without emitting expected absence errors.
    if let Ok(out) = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("name=^/{name}")])
        .output()
    {
        for id in String::from_utf8_lossy(&out.stdout).lines() {
            let _ = docker_remove(id);
        }
    }
}

/// Installs cleanup hooks so that the LocalStack container is removed both on normal
/// process exit (`atexit`) and on an interrupted exit (`SIGINT` / `SIGTERM`).
fn install_cleanup() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        unsafe { libc::atexit(cleanup) };
        crate::signal::register_signal_cleanup(|| cleanup());
    });
}

/// Picks a free TCP port on localhost by briefly binding to port 0, letting the OS
/// assign an available port, then releasing the bind before returning the port number.
///
/// There is a small inherent race window between the release and Docker binding the port,
/// but this is the standard approach and is reliable in practice.
fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("shouldn't fail binding to a random port")
        .local_addr()
        .expect("shouldn't fail reading local address")
        .port()
}

/// Spins up a LocalStack container with custom environment variables.
///
/// This function uses [`testcontainers`] to start a LocalStack Docker container with:
/// - Optional environment variables (e.g., AWS services to enable)
/// - Mounted Docker socket (for container-in-container support)
/// - A pre-emptively selected free host port mapped to container port 4566
///
/// It also sets up structured JSON tracing using `tracing_subscriber`.
///
/// # Arguments
///
/// * `env_vars` - A map of environment variables to pass to the LocalStack container.
///
/// # Returns
///
/// A tuple of the running [`ContainerAsync<LocalStackPro>`] instance and the host port it
/// is bound to, ready for AWS SDK interactions.
///
/// # Panics
///
/// Panics if the container fails to start.
pub async fn spin_up_localstack(
    env_vars: HashMap<&str, String>,
) -> (ContainerAsync<LocalStackPro>, u16) {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .try_init();
    debug!("Successfully initialized tracing_subscriber.");

    let port = find_free_port();

    let auth_token = std::env::var("LOCALSTACK_AUTH_TOKEN")
        .or_else(|_| std::env::var("LOCALSTACK_API_KEY"))
        .ok();

    let request = env_vars
        .iter()
        .fold(
            LocalStackPro::with_auth_token(auth_token)
                .with_container_name(localstack_container_name())
                .with_tag("latest"),
            |ls, (k, v)| ls.with_env_var(*k, v.as_str()),
        )
        .with_mount(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ))
        .with_mapped_port(port, LOCALSTACK_CONTAINER_PORT.tcp());

    let container = request
        .start()
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to start LocalStack.");
            e
        })
        .unwrap();
    debug!("Successfully started LocalStack-Container.");
    (container, port)
}

/// Spins up a LocalStack container with the specified AWS services enabled.
///
/// This is a convenience wrapper over [`spin_up_localstack`], which builds the `SERVICES`
/// environment variable string from the provided list.
///
/// # Arguments
///
/// * `services` - A list of AWS service identifiers (e.g., `"s3"`, `"dynamodb"`).
///
/// # Returns
///
/// A tuple of the running [`ContainerAsync<LocalStackPro>`] with only the requested services
/// enabled, and the host port it is bound to.
pub async fn spin_up_localstack_with_services(
    services: &[&str],
    extra_env_vars: &[(&str, &str)],
) -> (ContainerAsync<LocalStackPro>, u16) {
    let mut env_vars = HashMap::from([
        ("SERVICES", services.join(",")),
        (
            "LAMBDA_DOCKER_FLAGS",
            "--add-host=host.docker.internal:host-gateway".to_owned(),
        ),
        ("ENFORCE_IAM", "1".to_owned()),
    ]);
    for (k, v) in extra_env_vars {
        env_vars.insert(k, v.to_string());
    }
    spin_up_localstack(env_vars).await
}
