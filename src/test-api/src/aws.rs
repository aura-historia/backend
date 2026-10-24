mod topology;

// These library regressions share one process-owned fixture under unmanaged libtest.
#[cfg(all(
    test,
    any(
        feature = "opensearch",
        all(feature = "sqs", feature = "ses", feature = "s3")
    )
))]
pub(crate) const RUNTIME_TEST_SERVICES: &[&str] = &[
    #[cfg(feature = "opensearch")]
    "opensearch",
    #[cfg(all(feature = "sqs", feature = "ses", feature = "s3"))]
    "sqs",
    #[cfg(all(feature = "sqs", feature = "ses", feature = "s3"))]
    "sesv2",
    #[cfg(all(feature = "sqs", feature = "ses", feature = "s3"))]
    "s3",
];

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
        .args(["rm", "-f", "-v", name])
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
    (container, port)
}

#[cfg(all(test, feature = "sqs", feature = "ses", feature = "s3"))]
mod runtime_tests {
    use super::*;
    use crate::{Isolation, TestContext};
    use aws_sdk_account::error::ProvideErrorMetadata;
    use aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};
    use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    async fn s3_client() -> aws_sdk_s3::Client {
        aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::config::Builder::from(get_aws_config().await)
                .force_path_style(true)
                .build(),
        )
    }

    struct AwsResources {
        queue: String,
        bucket: String,
        sender: String,
        configuration_set: String,
    }

    async fn create_resources(context: &TestContext) -> AwsResources {
        let queue = context.queue_name("runtime", false).await.unwrap();
        let bucket = context.bucket_name("runtime").await.unwrap();
        let sender = context.register_sender("runtime").await.unwrap();
        let configuration_set = context.configuration_set_name("runtime").await.unwrap();
        let sqs = crate::get_sqs_client().await;
        let queue_url = sqs
            .create_queue()
            .queue_name(&queue)
            .send()
            .await
            .unwrap()
            .queue_url()
            .unwrap()
            .to_owned();
        sqs.send_message()
            .queue_url(queue_url)
            .message_body("runtime marker")
            .send()
            .await
            .unwrap();
        write_s3_marker(&bucket).await;
        send_ses_marker(&sender, &configuration_set).await;
        AwsResources {
            queue,
            bucket,
            sender,
            configuration_set,
        }
    }

    async fn write_s3_marker(bucket: &str) {
        let s3 = s3_client().await;
        s3.create_bucket()
            .bucket(bucket)
            .create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::EuCentral1)
                    .build(),
            )
            .send()
            .await
            .unwrap();
        s3.put_object()
            .bucket(bucket)
            .key("marker")
            .body("runtime marker".as_bytes().to_vec().into())
            .send()
            .await
            .unwrap();
    }

    async fn send_ses_marker(sender: &str, configuration_set: &str) {
        crate::verify_sender_identity(sender).await;
        let ses = crate::get_ses_client().await;
        ses.create_configuration_set()
            .configuration_set_name(configuration_set)
            .send()
            .await
            .unwrap();
        ses.send_email()
            .from_email_address(sender)
            .configuration_set_name(configuration_set)
            .destination(
                Destination::builder()
                    .to_addresses("runtime-recipient@example.test")
                    .build(),
            )
            .content(
                EmailContent::builder()
                    .simple(
                        Message::builder()
                            .subject(Content::builder().data("Runtime marker").build().unwrap())
                            .body(
                                Body::builder()
                                    .text(
                                        Content::builder().data("runtime marker").build().unwrap(),
                                    )
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .send()
            .await
            .unwrap();
    }

    async fn read_resources(resources: &AwsResources) {
        let AwsResources {
            queue,
            bucket,
            sender,
            ..
        } = resources;
        let sqs = crate::get_sqs_client().await;
        let queue_url = sqs
            .get_queue_url()
            .queue_name(queue)
            .send()
            .await
            .unwrap()
            .queue_url()
            .unwrap()
            .to_owned();
        let received = sqs
            .receive_message()
            .queue_url(queue_url)
            .send()
            .await
            .unwrap();
        assert_eq!(Some("runtime marker"), received.messages()[0].body());
        let object = s3_client()
            .await
            .get_object()
            .bucket(bucket)
            .key("marker")
            .send()
            .await
            .unwrap();
        assert_eq!(
            b"runtime marker",
            object.body.collect().await.unwrap().into_bytes().as_ref()
        );
        assert!(
            crate::get_ses_client()
                .await
                .get_email_identity()
                .email_identity(sender)
                .send()
                .await
                .unwrap()
                .verified_for_sending_status()
        );
        let emails = crate::get_sent_emails_from(sender).await;
        assert_eq!(1, emails.len());
        assert_eq!("Runtime marker", emails[0].subject);
    }

    async fn assert_resources_removed(resources: AwsResources) {
        let AwsResources {
            queue,
            bucket,
            sender,
            configuration_set,
        } = resources;
        let queue_error = crate::get_sqs_client()
            .await
            .get_queue_url()
            .queue_name(queue)
            .send()
            .await
            .unwrap_err();
        assert!(
            queue_error
                .as_service_error()
                .unwrap()
                .is_queue_does_not_exist()
        );
        let bucket_error = s3_client()
            .await
            .head_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_err();
        assert!(bucket_error.as_service_error().unwrap().is_not_found());
        let ses = crate::get_ses_client().await;
        let identity_error = ses
            .get_email_identity()
            .email_identity(&sender)
            .send()
            .await
            .unwrap_err();
        assert!(
            identity_error
                .as_service_error()
                .unwrap()
                .is_not_found_exception()
        );
        let set_error = ses
            .get_configuration_set()
            .configuration_set_name(configuration_set)
            .send()
            .await
            .unwrap_err();
        assert_eq!(
            Some("NotFoundException"),
            set_error.as_service_error().unwrap().code()
        );
        assert!(crate::get_sent_emails_from(&sender).await.is_empty());
    }

    #[test]
    fn should_own_aws_connections_across_setup_reads_and_cleanup_runtimes() {
        let setup = runtime();
        let (session, resources) = setup.block_on(async {
            get_aws_emulator(RUNTIME_TEST_SERVICES, &[]).await;
            assert!(
                get_aws_config().await.http_client().is_none(),
                "shared fixture configuration must not retain a runtime's HTTP connection pool"
            );
            let session = TestContext::acquire("AWS runtime lifetime", Isolation::Isolated)
                .await
                .unwrap();
            let context = session.context().clone();
            let resources = context.scope(create_resources(&context)).await;
            (session, resources)
        });
        drop(setup);

        let reads = runtime();
        reads.block_on(read_resources(&resources));
        drop(reads);

        let cleanup = runtime();
        cleanup.block_on(async {
            session.finish().await.unwrap();
            assert_resources_removed(resources).await;
        });
    }
}
