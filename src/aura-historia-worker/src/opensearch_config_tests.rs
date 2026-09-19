use super::*;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use rstest::rstest;
use std::{collections::BTreeMap, error::Error, fs, io::Write, path::PathBuf};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const TEST_CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt");

fn inputs(stage: &str, scope: WorkerScope) -> BTreeMap<&'static str, String> {
    let mut values: BTreeMap<_, _> = [
        (WORKER_STAGE_ENV, stage),
        (WORKER_SCOPE_ENV, scope.as_str()),
        (COMMIT_SHA_ENV, "d5bd9ca854e713b0c587528f02037211b2020fd4"),
        (queue::AWS_REGION_ENV, "eu-central-1"),
        ("POSTGRES_HOST", "postgres.example.test"),
        ("POSTGRES_DATABASE", "database_canary"),
        ("POSTGRES_USERNAME", "username_canary"),
        ("POSTGRES_PASSWORD", "password_canary"),
        ("POSTGRES_SSL_MODE", "verify-full"),
        ("POSTGRES_SSL_ROOT_CERT", TEST_CA),
        (
            OPENSEARCH_ENDPOINT_URL_ENV,
            "https://endpoint-canary.example.test:9200",
        ),
        (OPENSEARCH_USERNAME_ENV, "username_canary"),
        (OPENSEARCH_PASSWORD_ENV, "password_canary"),
        (OPENSEARCH_SSL_ROOT_CERT_ENV, TEST_CA),
        (VERTEX_AI_PROJECT_ID_ENV, "test-project"),
        (VERTEX_AI_LOCATION_ENV, "europe-west3"),
        (VERTEX_AI_MODEL_ENV, "test-model"),
        (S3_BUCKET_NAME_TEMPLATES_ENV, "test-templates"),
        (NOTIFICATION_EMAIL_FROM_ENV, "sender@example.test"),
        (NOTIFICATION_EMAIL_REPLY_TO_ENV, "reply@example.test"),
    ]
    .map(|(key, value)| (key, value.to_owned()))
    .into();
    values.insert(
        queue::WORKER_QUEUE_URL_ENV,
        format!(
            "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-{stage}",
            scope.as_str()
        ),
    );
    values
}

fn parse(
    values: &BTreeMap<&'static str, String>,
) -> Result<WorkerOpenSearchConfig, WorkerStartupConfigError> {
    opensearch_config(
        &mut |key| values.get(key).cloned(),
        values.get(WORKER_STAGE_ENV).map(String::as_str),
    )
}

fn assert_redacted(error: &(dyn Error + 'static)) {
    let mut current = Some(error);
    while let Some(error) = current {
        let rendered = format!("{error} {error:?} {error:#?}");
        for canary in [
            "endpoint-canary",
            "username_canary",
            "password_canary",
            "path_canary",
            "CERTIFICATE",
            TEST_CA,
        ] {
            assert!(
                !rendered.contains(canary),
                "configuration error leaked input"
            );
        }
        current = error.source();
    }
}

fn assert_tls_error(
    result: Result<WorkerOpenSearchConfig, WorkerStartupConfigError>,
    expected: OpenSearchTlsError,
) -> TestResult {
    let error = result.err().ok_or("expected OpenSearch config rejection")?;
    assert_redacted(&error);
    assert!(
        matches!(&error, WorkerStartupConfigError::OpenSearchTls(actual) if *actual == expected)
    );
    assert_eq!(
        error
            .source()
            .and_then(|cause| cause.downcast_ref::<OpenSearchTlsError>()),
        Some(&expected)
    );
    Ok(())
}

#[rstest]
fn should_freeze_trust_and_keep_auth_for_real_search_scopes(
    #[values("dev", "prod")] stage: &str,
    #[values(
        WorkerScope::SearchFilterProjection,
        WorkerScope::SearchFilterPercolator,
        WorkerScope::ProductListingOpenSearch
    )]
    scope: WorkerScope,
) -> TestResult {
    let values = inputs(stage, scope);
    let config = WorkerStartupConfig::from_getter(|key| values.get(key).cloned())?;
    let search = config.opensearch().ok_or("missing search config")?;
    assert_eq!(
        search.endpoint(),
        &url::Url::parse(&values[OPENSEARCH_ENDPOINT_URL_ENV])?
    );
    assert_eq!(
        search.basic_auth(),
        Some(("username_canary", "password_canary"))
    );
    assert_eq!(
        format!("{:?}", search.tls()),
        "OpenSearchTlsConfig { trust: [REDACTED] }"
    );
    Ok(())
}

#[rstest]
fn should_require_ca_for_each_real_search_scope(
    #[values("dev", "prod")] stage: &str,
    #[values(
        WorkerScope::SearchFilterProjection,
        WorkerScope::SearchFilterPercolator,
        WorkerScope::ProductListingOpenSearch
    )]
    scope: WorkerScope,
) -> TestResult {
    let mut values = inputs(stage, scope);
    values.remove(OPENSEARCH_SSL_ROOT_CERT_ENV);
    let error = WorkerStartupConfig::from_getter(|key| values.get(key).cloned())
        .err()
        .ok_or("expected missing CA rejection")?;
    assert_redacted(&error);
    assert!(matches!(
        error,
        WorkerStartupConfigError::OpenSearchTls(OpenSearchTlsError::MissingCa)
    ));
    Ok(())
}

#[rstest]
fn should_preserve_optional_ca_and_http_only_in_explicit_local_stages(
    #[values("local", "test", "ephemeral")] stage: &str,
    #[values("http", "https")] scheme: &str,
) -> TestResult {
    let mut values = inputs(stage, WorkerScope::SearchFilterProjection);
    values.insert(
        OPENSEARCH_ENDPOINT_URL_ENV,
        format!("{scheme}://localhost:9200"),
    );
    values.remove(OPENSEARCH_SSL_ROOT_CERT_ENV);
    let config = WorkerStartupConfig::from_getter(|key| values.get(key).cloned())?;
    assert!(
        config
            .opensearch()
            .ok_or("missing search config")?
            .basic_auth()
            .is_none()
    );
    values.insert(OPENSEARCH_SSL_ROOT_CERT_ENV, TEST_CA.into());
    if scheme == "http" {
        assert_tls_error(parse(&values), OpenSearchTlsError::CaWithHttp)?;
    } else {
        assert!(parse(&values).is_ok());
    }
    Ok(())
}

#[rstest]
fn should_never_read_search_inputs_for_unrelated_scopes(
    #[values(
        WorkerScope::SearchFilterMatchNotification,
        WorkerScope::WatchlistNotification,
        WorkerScope::ProductListingContentAssessment,
        WorkerScope::ProductListingTranslation,
        WorkerScope::ProductListingEmbedding,
        WorkerScope::ProductListingRawNormalization,
        WorkerScope::NotificationDelivery
    )]
    scope: WorkerScope,
) -> TestResult {
    let values = inputs("prod", scope);
    let config = WorkerStartupConfig::from_getter(|key| {
        assert!(
            !key.starts_with("OPENSEARCH_"),
            "unrelated scope read search input"
        );
        values.get(key).cloned()
    })?;
    assert!(config.opensearch().is_none());
    Ok(())
}

#[rstest]
#[case(None, OpenSearchTlsError::MissingCa)]
#[case(Some(""), OpenSearchTlsError::EmptyCaPath)]
#[case(Some(" \t\n"), OpenSearchTlsError::EmptyCaPath)]
#[case(Some("/nonexistent/path_canary/ca.pem"), OpenSearchTlsError::CaRead)]
#[case(Some(env!("CARGO_MANIFEST_DIR")), OpenSearchTlsError::CaNotRegular)]
fn should_reject_invalid_ca_paths_without_echoing_input(
    #[case] path: Option<&str>,
    #[case] expected: OpenSearchTlsError,
) -> TestResult {
    let mut values = inputs("prod", WorkerScope::SearchFilterProjection);
    values.remove(OPENSEARCH_SSL_ROOT_CERT_ENV);
    if let Some(path) = path {
        values.insert(OPENSEARCH_SSL_ROOT_CERT_ENV, path.into());
    }
    assert_tls_error(parse(&values), expected)
}

struct CaFile(PathBuf);
impl CaFile {
    fn new(bytes: &[u8]) -> TestResult<Self> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            ".opensearch-path_canary-{}.pem",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let fixture = Self(path);
        file.write_all(bytes)?;
        Ok(fixture)
    }

    fn path(&self) -> TestResult<&str> {
        self.0
            .to_str()
            .ok_or_else(|| "non-Unicode test fixture path".into())
    }
}
impl Drop for CaFile {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("OpenSearch CA fixture cleanup failed (path suppressed)");
        }
    }
}

#[rstest]
#[case(b"", OpenSearchTlsError::InvalidCa)]
#[case(b" \n", OpenSearchTlsError::InvalidCa)]
#[case(b"not PEM: password_canary", OpenSearchTlsError::InvalidCa)]
#[case(
    b"-----BEGIN CERTIFICATE-----\nY2FuYXJ5\n-----END CERTIFICATE-----\n",
    OpenSearchTlsError::InvalidCa
)]
#[case(&[b'x'; 1024 * 1024 + 1], OpenSearchTlsError::CaTooLarge)]
fn should_reject_empty_malformed_and_oversized_ca_files(
    #[case] bytes: &[u8],
    #[case] expected: OpenSearchTlsError,
) -> TestResult {
    let fixture = CaFile::new(bytes)?;
    let mut values = inputs("prod", WorkerScope::SearchFilterProjection);
    values.insert(OPENSEARCH_SSL_ROOT_CERT_ENV, fixture.path()?.into());
    assert_tls_error(parse(&values), expected)
}

#[rstest]
#[case(
    "prod",
    "http://endpoint-canary.example.test",
    OpenSearchTlsError::HttpsRequired
)]
#[case(
    "dev",
    "http://endpoint-canary.example.test",
    OpenSearchTlsError::HttpsRequired
)]
#[case(
    "local",
    "ftp://endpoint-canary.example.test",
    OpenSearchTlsError::HttpsRequired
)]
#[case(
    "prod",
    "https://username_canary:password_canary@endpoint-canary.example.test",
    OpenSearchTlsError::InvalidEndpoint
)]
#[case(
    "local",
    "http://username_canary@endpoint-canary.example.test",
    OpenSearchTlsError::InvalidEndpoint
)]
#[case(
    "prod",
    "https://endpoint-canary.example.test?password_canary",
    OpenSearchTlsError::InvalidEndpoint
)]
#[case(
    "local",
    "https://endpoint-canary.example.test#password_canary",
    OpenSearchTlsError::InvalidEndpoint
)]
#[case("prod", "file:///path_canary", OpenSearchTlsError::InvalidEndpoint)]
#[case(
    "",
    "https://endpoint-canary.example.test",
    OpenSearchTlsError::InvalidStage
)]
#[case(
    "DEV",
    "https://endpoint-canary.example.test",
    OpenSearchTlsError::InvalidStage
)]
#[case(
    " prod ",
    "https://endpoint-canary.example.test",
    OpenSearchTlsError::InvalidStage
)]
#[case(
    "unknown",
    "https://endpoint-canary.example.test",
    OpenSearchTlsError::InvalidStage
)]
fn should_reject_unsafe_urls_and_noncanonical_stages(
    #[case] stage: &str,
    #[case] endpoint: &str,
    #[case] expected: OpenSearchTlsError,
) -> TestResult {
    let mut values = inputs(stage, WorkerScope::SearchFilterProjection);
    values.insert(OPENSEARCH_ENDPOINT_URL_ENV, endpoint.into());
    assert_tls_error(parse(&values), expected)
}

#[test]
fn should_reject_missing_stage_and_malformed_url_without_echoing_input() -> TestResult {
    let mut values = inputs("prod", WorkerScope::SearchFilterProjection);
    values.remove(WORKER_STAGE_ENV);
    assert!(matches!(
        parse(&values),
        Err(WorkerStartupConfigError::MissingEnv {
            name: WORKER_STAGE_ENV
        })
    ));
    values.insert(WORKER_STAGE_ENV, "prod".into());
    values.insert(OPENSEARCH_ENDPOINT_URL_ENV, "password_canary".into());
    let error = parse(&values)
        .err()
        .ok_or("expected malformed URL rejection")?;
    assert_redacted(&error);
    assert!(matches!(
        error,
        WorkerStartupConfigError::InvalidOpenSearchEndpoint { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn should_build_both_clients_from_frozen_ca_after_file_replacement_and_removal() -> TestResult
{
    let fixture = CaFile::new(include_bytes!("postgres-test-ca.crt"))?;
    let mut values = inputs("prod", WorkerScope::SearchFilterProjection);
    values.insert(OPENSEARCH_SSL_ROOT_CERT_ENV, fixture.path()?.into());
    let mut ca_reads = 0;
    let startup = WorkerStartupConfig::from_getter(|key| {
        if key == OPENSEARCH_SSL_ROOT_CERT_ENV {
            ca_reads += 1;
        }
        values.get(key).cloned()
    })?;
    assert_eq!(ca_reads, 1);
    let config = startup.opensearch().ok_or("missing search config")?;
    let frozen = config.tls().clone();
    fs::write(&fixture.0, b"malformed replacement")?;
    assert_tls_error(parse(&values), OpenSearchTlsError::InvalidCa)?;
    for removed in [false, true] {
        if removed {
            fs::remove_file(&fixture.0)?;
            assert_tls_error(parse(&values), OpenSearchTlsError::CaRead)?;
        }
        for tls in [config.tls(), &frozen] {
            tls.configure_transport(TransportBuilder::new(SingleNodeConnectionPool::new(
                config.endpoint().clone(),
            )))?
            .build()?;
            tls.configure_http(reqwest::Client::builder()).build()?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_nonunicode_ca_only_for_search_scopes() -> TestResult {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command, time::Instant};
    const CHILD: &str = "WORKER_CA_NONUNICODE_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let result = WorkerStartupConfig::from_env();
        if std::env::var(WORKER_SCOPE_ENV)? == WorkerScope::SearchFilterProjection.as_str() {
            let error = result.err().ok_or("non-Unicode CA was ignored")?;
            assert_redacted(&error);
            assert!(matches!(
                error,
                WorkerStartupConfigError::OpenSearchTls(OpenSearchTlsError::EmptyCaPath)
            ));
        } else {
            assert!(result?.opensearch().is_none());
        }
        return Ok(());
    }
    for scope in [
        WorkerScope::SearchFilterProjection,
        WorkerScope::WatchlistNotification,
    ] {
        let mut child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "opensearch_config_tests::should_reject_nonunicode_ca_only_for_search_scopes",
                "--nocapture",
            ])
            .env_clear()
            .envs(inputs("test", scope))
            .env(CHILD, "1")
            .env(
                OPENSEARCH_SSL_ROOT_CERT_ENV,
                OsString::from_vec(b"path_canary\xff".to_vec()),
            )
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                assert!(status.success(), "isolated non-Unicode test failed");
                break;
            }
            if Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err("isolated non-Unicode test timed out".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Ok(())
}
