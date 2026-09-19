use super::*;
use platform_opensearch::tls::OpenSearchTlsError;
use std::error::Error;

const CA: &str = "OPENSEARCH_SSL_ROOT_CERT";
const PUBLIC_CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt");

fn stage_inputs(stage: &str) -> BTreeMap<&'static str, String> {
    let mut values = inputs();
    values.insert("STAGE", stage.into());
    values.insert("OPENSEARCH_USERNAME", "username_canary".into());
    values.insert("OPENSEARCH_PASSWORD", "password_canary".into());
    if matches!(stage, "dev" | "prod") {
        values.insert("POSTGRES_SSL_MODE", "verify-full".into());
        values.insert("POSTGRES_SSL_ROOT_CERT", PUBLIC_CA.into());
        values.insert(
            "COMMIT_SHA",
            "14fa8e7d80841d34ce69dd51e77a98e51a213c67".into(),
        );
        for name in [
            crate::COGNITO_JWKS_URL_ENV,
            crate::ZOHO_ACCOUNTS_URL_ENV,
            crate::ZOHO_CAMPAIGNS_URL_ENV,
        ] {
            values.insert(name, "https://127.0.0.1:1".into());
        }
    }
    values.insert("OPENSEARCH_ENDPOINT_URL", "https://127.0.0.1:1".into());
    values
}

fn assert_safe(error: &(dyn Error + 'static)) {
    let mut current = Some(error);
    while let Some(error) = current {
        let rendered = format!("{error} {error:?} {error:#?}");
        for secret in ["canary", PUBLIC_CA, "BEGIN CERTIFICATE"] {
            assert!(!rendered.contains(secret), "TLS error exposed input");
        }
        current = error.source();
    }
}

#[rstest::rstest]
#[case("dev")]
#[case("prod")]
fn should_require_valid_opensearch_ca_before_startup_io(#[case] stage: &str) -> TestResult {
    let directory = OwnedDirectory::new()?;
    let invalid = directory.path("value_canary.pem");
    let mut values = stage_inputs(stage);
    for (input, expected) in [
        (None, OpenSearchTlsError::MissingCa),
        (Some(""), OpenSearchTlsError::EmptyCaPath),
        (Some(" \t"), OpenSearchTlsError::EmptyCaPath),
        (
            Some("/missing/value_canary.pem"),
            OpenSearchTlsError::CaRead,
        ),
    ] {
        values.remove(CA);
        if let Some(value) = input {
            values.insert(CA, value.into());
        }
        let error = parse(&values).err().ok_or("invalid CA accepted")?;
        assert!(matches!(error, ApiStateError::OpenSearchTls(actual) if actual == expected));
        assert_safe(&error);
    }
    values.insert(
        CA,
        invalid.to_str().ok_or("non-Unicode fixture path")?.into(),
    );
    for bytes in [
        b"".as_slice(),
        b"value_canary",
        b"-----BEGIN CERTIFICATE-----\nvalue_canary\n-----END CERTIFICATE-----\n",
    ] {
        std::fs::write(&invalid, bytes)?;
        let error = parse(&values).err().ok_or("invalid PEM accepted")?;
        assert!(matches!(
            error,
            ApiStateError::OpenSearchTls(OpenSearchTlsError::InvalidCa)
        ));
        assert_safe(&error);
    }
    values.insert(CA, PUBLIC_CA.into());
    parse(&values)?;
    Ok(())
}

#[rstest::rstest]
#[case("local")]
#[case("test")]
#[case("ephemeral")]
fn should_allow_explicit_local_stage_tls_policy_without_changing_auth(
    #[case] stage: &str,
) -> TestResult {
    let mut values = stage_inputs(stage);
    parse(&values)?;
    values.insert(CA, PUBLIC_CA.into());
    parse(&values)?;
    values.insert("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:1".into());
    assert!(matches!(
        parse(&values),
        Err(ApiStateError::OpenSearchTls(OpenSearchTlsError::CaWithHttp))
    ));
    values.remove(CA);
    parse(&values)?;
    values.remove("OPENSEARCH_USERNAME");
    values.remove("OPENSEARCH_PASSWORD");
    assert_eq!(parse(&values).is_ok(), stage == "ephemeral");
    Ok(())
}

#[test]
fn should_reject_unsafe_opensearch_endpoints_and_inexact_stages() -> TestResult {
    for stage in ["dev", "prod", "local", "test", "ephemeral"] {
        let mut values = stage_inputs(stage);
        values.insert(CA, PUBLIC_CA.into());
        for endpoint in [
            "https://username_canary:password_canary@localhost",
            "https://localhost?value_canary",
            "https://localhost#value_canary",
            "ftp://localhost",
            "value_canary",
        ] {
            values.insert("OPENSEARCH_ENDPOINT_URL", endpoint.into());
            let error = parse(&values).err().ok_or("unsafe endpoint accepted")?;
            assert_safe(&error);
        }
        if matches!(stage, "dev" | "prod") {
            values.insert("OPENSEARCH_ENDPOINT_URL", "http://localhost".into());
            assert!(parse(&values).is_err());
        }
    }
    for stage in ["", "DEV", " dev", "test ", "value_canary"] {
        assert!(parse(&stage_inputs(stage)).is_err());
    }
    Ok(())
}

fn loopback_certificate() -> TestResult<(
    openssl::x509::X509,
    openssl::x509::X509,
    openssl::pkey::PKey<openssl::pkey::Private>,
)> {
    use openssl::{
        asn1::Asn1Time,
        hash::MessageDigest,
        pkey::PKey,
        rsa::Rsa,
        x509::{
            X509, X509NameBuilder,
            extension::{BasicConstraints, KeyUsage, SubjectAlternativeName},
        },
    };
    let key = PKey::from_rsa(Rsa::generate(2048)?)?;
    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_text("CN", "localhost")?;
    let name = name.build();
    let mut cert = X509::builder()?;
    cert.set_version(2)?;
    let serial = openssl::bn::BigNum::from_u32(1)?.to_asn1_integer()?;
    cert.set_serial_number(&serial)?;
    cert.set_subject_name(&name)?;
    cert.set_issuer_name(&name)?;
    cert.set_pubkey(&key)?;
    let not_before = Asn1Time::days_from_now(0)?;
    let not_after = Asn1Time::days_from_now(2)?;
    cert.set_not_before(&not_before)?;
    cert.set_not_after(&not_after)?;
    cert.append_extension(BasicConstraints::new().critical().ca().build()?)?;
    cert.append_extension(
        KeyUsage::new()
            .digital_signature()
            .key_encipherment()
            .key_cert_sign()
            .build()?,
    )?;
    cert.sign(&key, MessageDigest::sha256())?;
    let ca = cert.build();
    let server_key = PKey::from_rsa(Rsa::generate(2048)?)?;
    let mut cert = X509::builder()?;
    cert.set_version(2)?;
    let serial = openssl::bn::BigNum::from_u32(2)?.to_asn1_integer()?;
    cert.set_serial_number(&serial)?;
    cert.set_subject_name(&name)?;
    cert.set_issuer_name(ca.subject_name())?;
    cert.set_pubkey(&server_key)?;
    cert.set_not_before(&not_before)?;
    cert.set_not_after(&not_after)?;
    cert.append_extension(BasicConstraints::new().critical().build()?)?;
    cert.append_extension(
        KeyUsage::new()
            .digital_signature()
            .key_encipherment()
            .build()?,
    )?;
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .build(&cert.x509v3_context(Some(&ca), None))?;
    cert.append_extension(san)?;
    cert.sign(&key, MessageDigest::sha256())?;
    Ok((ca, cert.build(), server_key))
}

struct TlsWitness {
    address: SocketAddr,
    thread: Option<std::thread::JoinHandle<TestResult<bool>>>,
}

impl TlsWitness {
    fn start(
        cert: &openssl::x509::X509,
        key: &openssl::pkey::PKey<openssl::pkey::Private>,
    ) -> TestResult<Self> {
        use openssl::ssl::{SslAcceptor, SslMethod};
        use std::io::{Read, Write};
        let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls_server())?;
        acceptor.set_certificate(cert)?;
        acceptor.set_private_key(key)?;
        acceptor.check_private_key()?;
        let acceptor = acceptor.build();
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            socket.set_write_timeout(Some(Duration::from_secs(3)))?;
            let mut stream = match acceptor.accept(socket) {
                Ok(stream) => stream,
                Err(_) => return Ok(false),
            };
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                if request.len() >= 8192 || stream.read(&mut byte)? == 0 {
                    return Err("invalid TLS witness request".into());
                }
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"HEAD / HTTP/1.1\r\n"));
            let expected = format!(
                "authorization: Basic {}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    "username_canary:password_canary"
                )
            );
            assert!(
                String::from_utf8(request)?.contains(&expected),
                "runtime Basic auth missing"
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
            stream.flush()?;
            Ok(true)
        });
        Ok(Self {
            address,
            thread: Some(thread),
        })
    }

    fn finish(mut self) -> TestResult<bool> {
        self.thread
            .take()
            .ok_or("TLS witness join missing")?
            .join()
            .map_err(|_| "TLS witness panicked")?
    }
}

impl Drop for TlsWitness {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // Socket deadlines bound cleanup even if client/config/assertion fails first.
            if !matches!(thread.join(), Ok(Ok(_))) {
                eprintln!("owned TLS witness cleanup failed");
            }
        }
    }
}

#[tokio::test]
async fn should_use_frozen_ca_and_verify_peer_in_actual_startup_client() -> TestResult {
    let (ca, cert, key) = loopback_certificate()?;
    let directory = OwnedDirectory::new()?;
    let ca_path = directory.path("value_canary.pem");
    for (host, trust, success) in [
        ("localhost", true, true),
        ("127.0.0.1", true, false),
        ("localhost", false, false),
    ] {
        let server = TlsWitness::start(&cert, &key)?;
        std::fs::write(&ca_path, ca.to_pem()?)?;
        let mut values = stage_inputs("prod");
        values.insert(
            "OPENSEARCH_ENDPOINT_URL",
            format!("https://{host}:{}", server.address.port()),
        );
        values.insert(
            CA,
            if trust {
                ca_path.to_str().ok_or("non-Unicode fixture path")?
            } else {
                PUBLIC_CA
            }
            .into(),
        );
        let mut reads = 0;
        let api = ApiConfig::from_getter(|name| values.get(name).cloned())?;
        let config = StartupConfig::from_getter(api, &mut |name| {
            if name == CA {
                reads += 1;
            }
            values.get(name).cloned()
        })?;
        assert_eq!(reads, 1);
        std::fs::write(&ca_path, b"value_canary")?;
        let result = timeout(Duration::from_secs(4), check_search(&config.opensearch)).await?;
        let served = server.finish()?;
        assert_eq!(result.is_ok(), success);
        assert_eq!(served, success);
        if let Err(error) = result {
            assert_safe(&error);
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "secured OpenSearch image witness"]
async fn should_ping_secured_opensearch_from_witness_environment() -> TestResult {
    let api = ApiConfig::from_env()?;
    let config = StartupConfig::from_env(api)?;
    check_search(&config.opensearch).await?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_non_unicode_opensearch_ca_from_actual_environment() -> TestResult {
    use std::os::unix::ffi::OsStringExt;
    let directory = OwnedDirectory::new()?;
    let output = std::fs::File::create(directory.path("output"))?;
    let mut child = OwnedChild(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "runtime::tests::opensearch_tls::non_unicode_child",
                "--ignored",
            ])
            .env_clear()
            .env(
                CA,
                std::ffi::OsString::from_vec(b"value_canary\xff".to_vec()),
            )
            .stdout(output.try_clone()?)
            .stderr(output)
            .stdin(Stdio::null())
            .spawn()?,
    );
    assert!(child.wait(Duration::from_secs(5))?.success());
    let output = std::fs::read_to_string(directory.path("output"))?;
    assert!(output.contains("1 passed"));
    assert!(!output.contains("value_canary"));
    Ok(())
}

#[cfg(unix)]
#[test]
#[ignore = "parent-owned non-Unicode environment fixture"]
fn non_unicode_child() -> TestResult {
    assert!(matches!(
        std::env::var(CA),
        Err(std::env::VarError::NotUnicode(_))
    ));
    for stage in ["dev", "prod", "local", "test", "ephemeral"] {
        let values = stage_inputs(stage);
        let api = ApiConfig::from_getter(|key| values.get(key).cloned())?;
        let error = StartupConfig::from_getter(api, &mut |key| {
            if key == CA {
                env_input(key)
            } else {
                values.get(key).cloned()
            }
        })
        .err()
        .ok_or("non-Unicode CA accepted")?;
        assert!(matches!(
            error,
            ApiStateError::OpenSearchTls(OpenSearchTlsError::EmptyCaPath)
        ));
        assert_safe(&error);
    }
    Ok(())
}
