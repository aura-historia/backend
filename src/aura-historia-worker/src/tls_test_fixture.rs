// Included only by worker/cron private TLS tests; no runtime dependency or public test API.
use rustls::{
    AlertDescription, ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
};
use std::{
    fs,
    io::{self, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
pub(super) const REQUEST_BOUND: Duration = Duration::from_secs(4);
const IO_BOUND: Duration = Duration::from_secs(3);
pub(super) const IDENTITY: &str = r#"{"version":{"distribution":"opensearch","number":"3.1.0"}}"#;

pub(super) struct OwnedChild(Child);
impl OwnedChild {
    pub(super) fn run(command: &mut Command) -> TestResult {
        let mut child = Self(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("owned TLS fixture child failed".into())
                };
            }
            if Instant::now() >= deadline {
                return Err("owned TLS fixture child timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            if self.0.kill().is_err() {
                eprintln!("owned TLS child kill failed");
            }
            if self.0.wait().is_err() {
                eprintln!("owned TLS child reap failed");
            }
        }
    }
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0).is_err() {
            eprintln!("owned TLS fixture directory cleanup failed");
        }
    }
}

pub(super) struct Fixture {
    _directory: Directory,
    pub(super) ca_path: PathBuf,
    ca: Vec<u8>,
    server: Arc<ServerConfig>,
}
impl Fixture {
    pub(super) fn new() -> TestResult<Self> {
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "aura-loopback-tls-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        // Acquire cleanup before certificate generation can fail.
        let owned = Directory(directory);
        let openssl = |args: &[&str]| {
            OwnedChild::run(Command::new("openssl").current_dir(&owned.0).args(args))
        };
        openssl(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-noenc",
            "-days",
            "2",
            "-subj",
            "/CN=loopback-test-ca",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-keyout",
            "ca.key",
            "-out",
            "ca.pem",
        ])?;
        openssl(&[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-noenc",
            "-subj",
            "/CN=loopback-test-server",
            "-keyout",
            "server.key",
            "-out",
            "server.csr",
        ])?;
        fs::write(owned.0.join("server.ext"), b"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\n")?;
        openssl(&[
            "x509",
            "-req",
            "-in",
            "server.csr",
            "-CA",
            "ca.pem",
            "-CAkey",
            "ca.key",
            "-set_serial",
            "2",
            "-days",
            "2",
            "-extfile",
            "server.ext",
            "-out",
            "server.pem",
        ])?;
        let cert = fs::read(owned.0.join("server.pem"))?;
        let key = fs::read(owned.0.join("server.key"))?;
        let server =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from_pem_slice(&cert)?],
                    PrivateKeyDer::from_pem_slice(&key)?,
                )?;
        let ca = fs::read(owned.0.join("ca.pem"))?;
        let ca_path = owned.0.join("input.pem");
        fs::write(&ca_path, &ca)?;
        // Keys are never needed on disk by the listener.
        for name in [
            "ca.key",
            "server.key",
            "server.csr",
            "server.ext",
            "server.pem",
        ] {
            fs::remove_file(owned.0.join(name))?;
        }
        Ok(Self {
            _directory: owned,
            ca_path,
            ca,
            server: Arc::new(server),
        })
    }

    pub(super) fn trust(&self, trusted: bool) -> TestResult {
        fs::write(
            &self.ca_path,
            if trusted {
                &self.ca
            } else {
                include_bytes!("postgres-test-ca.crt").as_slice()
            },
        )?;
        Ok(())
    }

    pub(super) fn finish(self) -> TestResult {
        let directory = self._directory.0.clone();
        drop(self);
        if directory.try_exists()? {
            return Err("owned TLS directory remained after cleanup".into());
        }
        Ok(())
    }

    pub(super) fn listen(
        &self,
        method: &'static str,
        identity: &'static str,
    ) -> TestResult<Loopback> {
        Loopback::start(self.server.clone(), method, identity)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Responded,
    CertificateRejected,
}

pub(super) struct Loopback {
    port: u16,
    thread: Option<JoinHandle<io::Result<Outcome>>>,
}
impl Loopback {
    fn start(
        config: Arc<ServerConfig>,
        method: &'static str,
        identity: &'static str,
    ) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => {
                        return Err(io::Error::other("TLS fixture accept failed or timed out"));
                    }
                }
            };
            // Nonblocking TLS I/O lets the absolute deadline bound handshake and
            // partial headers too, rather than restarting a socket timeout per read.
            socket.set_nonblocking(true)?;
            let connection = ServerConnection::new(config)
                .map_err(|_| io::Error::other("TLS fixture configuration failed"))?;
            let mut stream = StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            let mut byte = [0];
            let deadline = Instant::now() + IO_BOUND;
            while !request.ends_with(b"\r\n\r\n") {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("TLS fixture request deadline"));
                }
                if request.len() >= 8192 {
                    return Err(io::Error::other("TLS fixture oversized request"));
                }
                match stream.read(&mut byte) {
                    Ok(0) => return Err(io::Error::other("TLS fixture unexpected EOF")),
                    Ok(_) => request.push(byte[0]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => {
                        // Only a peer certificate alert proves TLS rejection. EOF, reset and timeout fail.
                        let certificate_alert = matches!(
                            error
                                .get_ref()
                                .and_then(|source| source.downcast_ref::<rustls::Error>()),
                            Some(rustls::Error::AlertReceived(
                                AlertDescription::UnknownCA
                                    | AlertDescription::BadCertificate
                                    | AlertDescription::CertificateUnknown
                            ))
                        );
                        return if request.is_empty() && certificate_alert {
                            Ok(Outcome::CertificateRejected)
                        } else {
                            Err(io::Error::other(
                                "TLS fixture failed without certificate alert",
                            ))
                        };
                    }
                }
            }
            if !request.starts_with(format!("{method} / HTTP/1.1\r\n").as_bytes()) {
                return Err(io::Error::other("TLS fixture unexpected method or path"));
            }
            let headers = String::from_utf8(request)
                .map_err(|_| io::Error::other("TLS fixture invalid headers"))?;
            if !headers.lines().any(|line| {
                line.eq_ignore_ascii_case(
                    "authorization: Basic dXNlcm5hbWVfY2FuYXJ5OnBhc3N3b3JkX2NhbmFyeQ==",
                )
            }) {
                return Err(io::Error::other("TLS fixture missing synthetic Basic auth"));
            }
            let body = if method == "HEAD" { "" } else { identity };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                .map_err(|_| io::Error::other("TLS fixture response failed"))?;
            stream
                .flush()
                .map_err(|_| io::Error::other("TLS fixture flush failed"))?;
            Ok(Outcome::Responded)
        });
        Ok(Self {
            port,
            thread: Some(thread),
        })
    }

    pub(super) fn endpoint(&self, correct_hostname: bool) -> String {
        // IP SAN succeeds without DNS; localhost deliberately mismatches that SAN.
        let host = if correct_hostname {
            "127.0.0.1"
        } else {
            "localhost"
        };
        format!("https://{host}:{}/", self.port)
    }

    pub(super) fn finish(mut self) -> TestResult<Outcome> {
        self.thread
            .take()
            .ok_or("TLS fixture join missing")?
            .join()
            .map_err(|_| "TLS fixture thread panicked")?
            .map_err(Into::into)
    }
}
impl Drop for Loopback {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            if !matches!(thread.join(), Ok(Ok(_))) {
                eprintln!("owned TLS listener cleanup failed");
            }
        }
    }
}
