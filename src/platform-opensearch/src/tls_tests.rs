use super::*;
use opensearch::{OpenSearch, http::transport::SingleNodeConnectionPool};
use rustls::{ServerConfig, ServerConnection, StreamOwned, pki_types::PrivateKeyDer};
use std::{
    error::Error,
    fs,
    io::{self, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn Error>>;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/tls-tests");
        fs::create_dir_all(&parent)?;
        let path = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn file(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        fs::write(&path, bytes)?;
        Ok(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!("TLS fixture cleanup failed: {:?}", error.kind());
        }
    }
}

fn path_str(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str()
        .ok_or_else(|| "fixture path is not UTF-8".into())
}

fn openssl(directory: &TestDirectory, args: &[&str]) -> io::Result<()> {
    let status = Command::new("timeout")
        .args(["10s", "openssl"])
        .args(args)
        .current_dir(&directory.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("bounded OpenSSL fixture command failed"))
    }
}

struct Fixture {
    ca: Vec<u8>,
    other_ca: Vec<u8>,
    server: Arc<ServerConfig>,
    rotated_server: Arc<ServerConfig>,
}

impl Fixture {
    fn generate() -> Result<Self, Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        for name in ["ca", "other"] {
            openssl(
                &directory,
                &[
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-noenc",
                    "-days",
                    "2",
                    "-subj",
                    &format!("/CN=loopback-{name}"),
                    "-addext",
                    "basicConstraints=critical,CA:TRUE",
                    "-addext",
                    "keyUsage=critical,keyCertSign,cRLSign",
                    "-keyout",
                    &format!("{name}.key"),
                    "-out",
                    &format!("{name}.pem"),
                ],
            )?;
        }
        openssl(
            &directory,
            &[
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-noenc",
                "-subj",
                "/CN=localhost",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
            ],
        )?;
        directory.file("server.ext", b"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost\n")?;
        for name in ["ca", "other"] {
            openssl(
                &directory,
                &[
                    "x509",
                    "-req",
                    "-in",
                    "server.csr",
                    "-CA",
                    &format!("{name}.pem"),
                    "-CAkey",
                    &format!("{name}.key"),
                    "-set_serial",
                    "2",
                    "-days",
                    "2",
                    "-extfile",
                    "server.ext",
                    "-out",
                    &format!("server-{name}.pem"),
                ],
            )?;
        }
        let server_config = |name: &str| -> Result<Arc<ServerConfig>, Box<dyn Error>> {
            let pem = fs::read(directory.0.join(format!("server-{name}.pem")))?;
            let key = fs::read(directory.0.join("server.key"))?;
            let config = ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from_pem_slice(&pem)?],
                PrivateKeyDer::from_pem_slice(&key)?,
            )?;
            Ok(Arc::new(config))
        };
        Ok(Self {
            ca: fs::read(directory.0.join("ca.pem"))?,
            other_ca: fs::read(directory.0.join("other.pem"))?,
            server: server_config("ca")?,
            rotated_server: server_config("other")?,
        })
    }
}

fn fixture() -> Result<&'static Fixture, Box<dyn Error>> {
    // Generate once per test process; keys never committed and disk files removed
    // immediately after loading. No downloads, cloud, Docker, or certificate expiry drift.
    static FIXTURE: OnceLock<Result<Fixture, String>> = OnceLock::new();
    match FIXTURE.get_or_init(|| {
        Fixture::generate()
            .map_err(|_| "TLS fixture generation failed; needs openssl and timeout".to_owned())
    }) {
        Ok(fixture) => Ok(fixture),
        Err(error) => Err(error.clone().into()),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ServerOutcome {
    Responded,
    TlsRejected,
}

struct Loopback {
    address: SocketAddr,
    thread: Option<JoinHandle<io::Result<ServerOutcome>>>,
}

impl Loopback {
    fn start(config: Arc<ServerConfig>, response: &'static [u8]) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + IO_TIMEOUT;
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            socket.set_read_timeout(Some(IO_TIMEOUT))?;
            socket.set_write_timeout(Some(IO_TIMEOUT))?;
            let mut connection = ServerConnection::new(config).map_err(io::Error::other)?;
            while connection.is_handshaking() {
                if let Err(error) = connection.complete_io(&mut socket) {
                    return if error.kind() == io::ErrorKind::InvalidData {
                        Ok(ServerOutcome::TlsRejected)
                    } else {
                        Err(error)
                    };
                }
            }
            let mut stream = StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                if request.len() >= 8192 || stream.read(&mut byte)? == 0 {
                    return Err(io::Error::other("invalid loopback HTTP request"));
                }
                request.push(byte[0]);
            }
            stream.write_all(response)?;
            stream.flush()?;
            Ok(ServerOutcome::Responded)
        });
        Ok(Self {
            address,
            thread: Some(thread),
        })
    }

    fn endpoint(&self, hostname: bool) -> Result<Url, url::ParseError> {
        let host = if hostname { "localhost" } else { "127.0.0.1" };
        Url::parse(&format!("https://{host}:{}/", self.address.port()))
    }

    fn finish(mut self) -> Result<ServerOutcome, Box<dyn Error>> {
        self.thread
            .take()
            .ok_or("TLS server join missing")?
            .join()
            .map_err(|_| "TLS server thread failed")?
            .map_err(Into::into)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // Existing accept/socket deadlines bound cleanup after early test failure.
            if !matches!(thread.join(), Ok(Ok(_))) {
                eprintln!("owned TLS loopback cleanup failed");
            }
        }
    }
}

const OK_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";

async fn check_both_clients(
    config: &OpenSearchTlsConfig,
    server_config: Arc<ServerConfig>,
    hostname: bool,
    success: bool,
) -> TestResult {
    for sdk in [false, true] {
        let server = Loopback::start(server_config.clone(), OK_RESPONSE)?;
        let endpoint = server.endpoint(hostname)?;
        // Successful requests also prove that explicit proxies and prior insecure
        // builder settings are replaced. Bounds belong to tests, not shared policy.
        let passed = if sdk {
            let builder = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
                .proxy(Url::parse("http://127.0.0.1:9")?, None, None)
                .cert_validation(CertificateValidation::None)
                .timeout(IO_TIMEOUT);
            let client = OpenSearch::new(config.configure_transport(builder)?.build()?);
            match tokio::time::timeout(IO_TIMEOUT, client.info().send()).await? {
                Ok(response) => response.status_code().is_success(),
                Err(_) => false,
            }
        } else {
            let builder = reqwest::Client::builder()
                .proxy(reqwest::Proxy::all("http://127.0.0.1:9")?)
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true)
                .timeout(IO_TIMEOUT);
            let client = config.configure_http(builder).build()?;
            match tokio::time::timeout(IO_TIMEOUT, client.get(endpoint).send()).await? {
                Ok(response) => response.status().is_success(),
                Err(error) => {
                    assert!(error.is_connect(), "expected TLS connection rejection");
                    false
                }
            }
        };
        assert_eq!(success, passed, "SDK client: {sdk}");
        assert_eq!(
            server.finish()?,
            if success {
                ServerOutcome::Responded
            } else {
                ServerOutcome::TlsRejected
            }
        );
    }
    Ok(())
}

#[test]
fn should_enforce_exact_stages_and_https_policy() -> TestResult {
    let https = Url::parse("https://localhost:9200")?;
    let http = Url::parse("http://localhost:9200")?;
    for stage in ["dev", "prod"] {
        assert_eq!(
            OpenSearchTlsConfig::from_inputs(stage, &https, None).err(),
            Some(OpenSearchTlsError::MissingCa)
        );
        assert_eq!(
            OpenSearchTlsConfig::from_inputs(stage, &http, None).err(),
            Some(OpenSearchTlsError::HttpsRequired)
        );
    }
    for stage in ["", "DEV", "production", " local", "local ", "unknown"] {
        assert_eq!(
            OpenSearchTlsConfig::from_inputs(stage, &https, None).err(),
            Some(OpenSearchTlsError::InvalidStage)
        );
    }
    for stage in ["local", "test", "ephemeral"] {
        assert!(OpenSearchTlsConfig::from_inputs(stage, &https, None).is_ok());
        assert!(OpenSearchTlsConfig::from_inputs(stage, &http, None).is_ok());
        for path in ["", "missing", " "] {
            assert_eq!(
                OpenSearchTlsConfig::from_inputs(stage, &http, Some(path)).err(),
                Some(OpenSearchTlsError::CaWithHttp)
            );
        }
        assert_eq!(
            OpenSearchTlsConfig::from_inputs(stage, &Url::parse("ftp://localhost")?, None).err(),
            Some(OpenSearchTlsError::HttpsRequired)
        );
    }
    Ok(())
}

#[test]
fn should_reject_endpoint_decorations_and_empty_ca_paths() -> TestResult {
    for endpoint in [
        "https://user@localhost",
        "https://:password@localhost",
        "https://user:password@localhost",
        "https://localhost?",
        "https://localhost?secret=value",
        "https://localhost#",
        "https://localhost#secret",
        "file:///tmp/search",
        "mailto:search@example.com",
    ] {
        assert_eq!(
            OpenSearchTlsConfig::from_inputs("local", &Url::parse(endpoint)?, None).err(),
            Some(OpenSearchTlsError::InvalidEndpoint)
        );
    }
    let endpoint = Url::parse("https://localhost")?;
    for path in ["", " ", "\t\n"] {
        for stage in ["local", "dev", "prod", "test", "ephemeral"] {
            assert_eq!(
                OpenSearchTlsConfig::from_inputs(stage, &endpoint, Some(path)).err(),
                Some(OpenSearchTlsError::EmptyCaPath)
            );
        }
    }
    Ok(())
}

#[test]
fn should_reject_unreadable_nonregular_and_oversized_ca_files() -> TestResult {
    let directory = TestDirectory::new()?;
    let endpoint = Url::parse("https://localhost")?;
    let missing = directory.0.join("sensitive-missing-ca");
    assert_eq!(
        OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&missing)?)).err(),
        Some(OpenSearchTlsError::CaRead)
    );
    assert_eq!(
        OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&directory.0)?)).err(),
        Some(OpenSearchTlsError::CaNotRegular)
    );
    let path = directory.file("large.pem", b"")?;
    fs::File::options()
        .write(true)
        .open(&path)?
        .set_len(MAX_CA_BYTES + 1)?;
    assert_eq!(
        OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&path)?)).err(),
        Some(OpenSearchTlsError::CaTooLarge)
    );
    #[cfg(unix)]
    {
        let fifo = directory.0.join("ca.fifo");
        let status = Command::new("timeout")
            .args(["3s", "mkfifo"])
            .arg(&fifo)
            .status()?;
        assert!(status.success());
        let started = Instant::now();
        assert_eq!(
            OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&fifo)?)).err(),
            Some(OpenSearchTlsError::CaNotRegular)
        );
        assert!(started.elapsed() < IO_TIMEOUT);
    }
    Ok(())
}

#[test]
fn should_reject_garbage_keys_empty_and_malformed_bundles_before_either_client() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let endpoint = Url::parse("https://localhost")?;
    let key = b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
    for bytes in [
        Vec::new(),
        b" \r\n".to_vec(),
        b"garbage".to_vec(),
        vec![0xff],
        key.to_vec(),
        b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_vec(),
        b"-----BEGIN CERTIFICATE-----\n!invalid!\n-----END CERTIFICATE-----\n".to_vec(),
        b"-----BEGIN CERTIFICATE-----\nAAAA\n".to_vec(),
        [fixture.ca.as_slice(), key].concat(),
        [key, fixture.ca.as_slice()].concat(),
        [fixture.ca.as_slice(), b"garbage"].concat(),
        [b"garbage", fixture.ca.as_slice()].concat(),
        [
            fixture.ca.as_slice(),
            b"-----BEGIN CERTIFICATE-----\nAAAA\n",
        ]
        .concat(),
    ] {
        let path = directory.file("invalid.pem", &bytes)?;
        for stage in ["local", "test", "ephemeral", "dev", "prod"] {
            assert_eq!(
                OpenSearchTlsConfig::from_inputs(stage, &endpoint, Some(path_str(&path)?)).err(),
                Some(OpenSearchTlsError::InvalidCa)
            );
        }
    }
    Ok(())
}

#[test]
fn should_accept_bounded_rotation_bundles_and_redact_config_and_errors() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let endpoint = Url::parse("https://localhost")?;
    let mut bundle = [b" \n".as_slice(), &fixture.ca, b"\n\t ", &fixture.other_ca].concat();
    bundle.resize(MAX_CA_BYTES as usize, b' ');
    let path = directory.file("sensitive-ca-path.pem", &bundle)?;
    for stage in ["dev", "prod", "local", "test", "ephemeral"] {
        let config = OpenSearchTlsConfig::from_inputs(stage, &endpoint, Some(path_str(&path)?))?;
        assert_eq!(config.http_roots.len(), 2);
        let sdk = Certificate::from_pem(config.pem.as_deref().ok_or("missing frozen CA")?)?;
        assert_eq!(sdk.len(), 2);
        assert_eq!(
            format!("{config:?}"),
            "OpenSearchTlsConfig { trust: [REDACTED] }"
        );
        assert_eq!(format!("{:?}", config.clone()), format!("{config:?}"));
    }
    for error in [
        OpenSearchTlsError::InvalidStage,
        OpenSearchTlsError::InvalidEndpoint,
        OpenSearchTlsError::HttpsRequired,
        OpenSearchTlsError::CaWithHttp,
        OpenSearchTlsError::MissingCa,
        OpenSearchTlsError::EmptyCaPath,
        OpenSearchTlsError::CaRead,
        OpenSearchTlsError::CaNotRegular,
        OpenSearchTlsError::CaTooLarge,
        OpenSearchTlsError::InvalidCa,
        OpenSearchTlsError::UnsupportedPlatform,
    ] {
        assert!(error.source().is_none());
        let formatted = format!("{error} {error:?}");
        assert!(!formatted.contains(path_str(&path)?));
        assert!(!formatted.contains("BEGIN CERTIFICATE"));
    }
    Ok(())
}

#[test]
fn should_accept_crlf_certificates_and_regular_mounted_secret_symlinks() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let pem = std::str::from_utf8(&fixture.ca)?.replace('\n', "\r\n");
    let path = directory.file("ca.pem", pem.as_bytes())?;
    let endpoint = Url::parse("https://localhost")?;
    let config = OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&path)?))?;
    assert_eq!(config.http_roots.len(), 1);
    assert_eq!(
        Certificate::from_pem(config.pem.as_deref().ok_or("missing frozen CA")?)?.len(),
        1
    );
    #[cfg(unix)]
    {
        let link = directory.0.join("mounted-ca.pem");
        std::os::unix::fs::symlink(&path, &link)?;
        let mounted = OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&link)?))?;
        assert_eq!(mounted.pem, config.pem);
    }
    Ok(())
}

#[tokio::test]
async fn should_verify_ca_hostname_and_missing_trust_through_both_real_clients() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let endpoint = Url::parse("https://localhost")?;
    let path = directory.file("ca.pem", &fixture.ca)?;
    let config = OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&path)?))?;
    // Freeze test: mutation and removal must not affect configured or cloned clients.
    fs::write(&path, b"invalid replacement")?;
    fs::remove_file(&path)?;
    check_both_clients(&config.clone(), fixture.server.clone(), true, true).await?;
    check_both_clients(&config, fixture.server.clone(), false, false).await?;
    let path = directory.file("wrong.pem", &fixture.other_ca)?;
    let wrong = OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&path)?))?;
    check_both_clients(&wrong, fixture.server.clone(), true, false).await?;
    for stage in ["local", "test", "ephemeral"] {
        let missing = OpenSearchTlsConfig::from_inputs(stage, &endpoint, None)?;
        check_both_clients(&missing, fixture.server.clone(), true, false).await?;
    }
    Ok(())
}

#[tokio::test]
async fn should_trust_both_rotation_cas_through_both_real_clients() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let bundle = [b" \n".as_slice(), &fixture.ca, b"\n\t ", &fixture.other_ca].concat();
    let path = directory.file("rotation.pem", &bundle)?;
    let config = OpenSearchTlsConfig::from_inputs(
        "prod",
        &Url::parse("https://localhost")?,
        Some(path_str(&path)?),
    )?;
    check_both_clients(&config, fixture.server.clone(), true, true).await?;
    check_both_clients(&config, fixture.rotated_server.clone(), true, true).await?;
    Ok(())
}

#[tokio::test]
async fn should_preserve_no_redirect_for_direct_http() -> TestResult {
    let fixture = fixture()?;
    let directory = TestDirectory::new()?;
    let path = directory.file("ca.pem", &fixture.ca)?;
    let server = Loopback::start(fixture.server.clone(), b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/never-follow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
    let endpoint = server.endpoint(true)?;
    let config = OpenSearchTlsConfig::from_inputs("prod", &endpoint, Some(path_str(&path)?))?;
    let client = config
        .configure_http(reqwest::Client::builder().timeout(IO_TIMEOUT))
        .build()?;
    let response = tokio::time::timeout(IO_TIMEOUT, client.get(endpoint).send()).await??;
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    assert_eq!(server.finish()?, ServerOutcome::Responded);
    Ok(())
}
