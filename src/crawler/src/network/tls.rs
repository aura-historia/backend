use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::{Arc, LazyLock};

/// Certificates are verified against the operating system's trust store, the
/// same source Spider's reqwest 0.13 client uses, so scraper and spider accept
/// the same shops. The `webpki-roots` bundle behind reqwest 0.12's
/// `rustls-tls` feature lags the OS stores: a shop whose chain ends at a newer
/// root fails with `UnknownIssuer` there.
///
/// Built once because every crawler request builds a fresh pinned client and
/// the Linux verifier reads the system CA bundle when it is constructed.
static PLATFORM_TLS_CONFIG: LazyLock<Result<rustls::ClientConfig, rustls::Error>> =
    LazyLock::new(|| {
        let config = build_platform_tls_config();
        if let Err(error) = &config {
            tracing::error!(
                error = %error,
                "Platform TLS trust store is unavailable; crawler HTTPS requests will fail"
            );
        }
        config
    });

pub fn platform_tls_config() -> Result<rustls::ClientConfig, rustls::Error> {
    PLATFORM_TLS_CONFIG.clone()
}

fn build_platform_tls_config() -> Result<rustls::ClientConfig, rustls::Error> {
    // reqwest 0.12's own fallback when no process-wide provider is installed.
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_platform_verifier()?
        .with_no_client_auth();
    // reqwest leaves a preconfigured config's ALPN alone; crawler clients are
    // HTTP/1.1 only.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}
