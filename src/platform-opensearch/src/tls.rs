//! Shared TLS policy for OpenSearch SDK and direct HTTP composition roots.
//!
//! Call with the same endpoint used by the client. Apply to fresh builders, then build;
//! callers must not override trust, proxy, redirect, or TLS settings afterwards.
//! Trust is additive to built-in roots, not exclusive CA pinning. SDK 2.4 has no
//! redirect policy setter; only the direct HTTP client disables redirects.

use opensearch::{
    cert::{Certificate, CertificateValidation},
    http::transport::TransportBuilder,
};
use rustls::{
    RootCertStore,
    pki_types::{CertificateDer, pem::PemObject},
};
use std::{fmt, fs::OpenOptions, io::Read, sync::Arc};
use url::Url;

const MAX_CA_BYTES: u64 = 1024 * 1024;

/// Validated, frozen CA input. No environment reads; reload by constructing a new config.
#[derive(Clone)]
pub struct OpenSearchTlsConfig {
    pem: Option<Arc<[u8]>>,
    http_roots: Vec<reqwest::Certificate>,
}

impl fmt::Debug for OpenSearchTlsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpenSearchTlsConfig { trust: [REDACTED] }")
    }
}

/// Safe configuration failures: no paths, endpoint, certificate bytes, or source chains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpenSearchTlsError {
    #[error("OpenSearch stage must be dev, prod, local, test, or ephemeral")]
    InvalidStage,
    #[error("OpenSearch endpoint must have a host and no userinfo, query, or fragment")]
    InvalidEndpoint,
    #[error("OpenSearch requires HTTPS outside explicit local/test/ephemeral stages")]
    HttpsRequired,
    #[error("OpenSearch CA input cannot be used with HTTP")]
    CaWithHttp,
    #[error("OpenSearch CA path is required outside explicit local/test/ephemeral stages")]
    MissingCa,
    #[error("OpenSearch CA path must not be empty")]
    EmptyCaPath,
    #[error("OpenSearch CA file could not be read")]
    CaRead,
    #[error("OpenSearch CA input must be a regular file")]
    CaNotRegular,
    #[error("OpenSearch CA file exceeds 1 MiB")]
    CaTooLarge,
    #[error("OpenSearch CA input must be a nonempty PEM certificate bundle")]
    InvalidCa,
    #[error("OpenSearch CA file loading is unsupported on this platform")]
    UnsupportedPlatform,
}

impl OpenSearchTlsConfig {
    /// Explicit `local`, `test`, and `ephemeral` permit HTTP or absent CA input.
    /// `dev` and `prod` require HTTPS and an explicit CA file. Supplied CA is never ignored.
    pub fn from_inputs(
        stage: &str,
        endpoint: &Url,
        ca_path: Option<&str>,
    ) -> Result<Self, OpenSearchTlsError> {
        let is_local_stage = match stage {
            "local" | "test" | "ephemeral" => true,
            "dev" | "prod" => false,
            _ => return Err(OpenSearchTlsError::InvalidStage),
        };
        if endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(OpenSearchTlsError::InvalidEndpoint);
        }
        match endpoint.scheme() {
            "https" => {}
            "http" if is_local_stage => {
                if ca_path.is_some() {
                    return Err(OpenSearchTlsError::CaWithHttp);
                }
            }
            _ => return Err(OpenSearchTlsError::HttpsRequired),
        }
        let Some(path) = ca_path else {
            return if is_local_stage {
                Ok(Self {
                    pem: None,
                    http_roots: Vec::new(),
                })
            } else {
                Err(OpenSearchTlsError::MissingCa)
            };
        };
        if path.trim().is_empty() {
            return Err(OpenSearchTlsError::EmptyCaPath);
        }
        let bytes = read_ca(path)?;
        let (pem, http_roots) = validate_bundle(&bytes)?;
        // Audit the actual SDK parser too. It is permissive by itself, so strict
        // PEM/DER validation above must precede this call.
        Certificate::from_pem(&pem).map_err(|_| OpenSearchTlsError::InvalidCa)?;
        Ok(Self {
            pem: Some(pem.into()),
            http_roots,
        })
    }

    /// SDK Certificate is not Clone. Recreate it from frozen, already validated bytes;
    /// never reread the file. SDK 2.4 Full adds roots and retains hostname validation.
    pub fn configure_transport(
        &self,
        builder: TransportBuilder,
    ) -> Result<TransportBuilder, OpenSearchTlsError> {
        let validation = match &self.pem {
            Some(pem) => CertificateValidation::Full(
                Certificate::from_pem(pem).map_err(|_| OpenSearchTlsError::InvalidCa)?,
            ),
            None => CertificateValidation::Default,
        };
        Ok(builder.disable_proxy().cert_validation(validation))
    }

    /// Add the same roots to workspace reqwest 0.12, retaining certificate/hostname
    /// verification and built-in roots. Disable all proxies and automatic redirects.
    /// No timeout policy is imposed here.
    pub fn configure_http(&self, builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        self.http_roots.iter().cloned().fold(
            builder
                .use_rustls_tls()
                .danger_accept_invalid_certs(false)
                .danger_accept_invalid_hostnames(false)
                .tls_built_in_root_certs(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none()),
            |builder, root| builder.add_root_certificate(root),
        )
    }
}

fn read_ca(path: &str) -> Result<Vec<u8>, OpenSearchTlsError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Check the opened descriptor, not a racy path stat. NONBLOCK prevents FIFO
        // opens from hanging; symlinks to regular mounted secrets remain supported.
        options.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    return Err(OpenSearchTlsError::UnsupportedPlatform);

    let file = options.open(path).map_err(|_| OpenSearchTlsError::CaRead)?;
    let metadata = file.metadata().map_err(|_| OpenSearchTlsError::CaRead)?;
    if !metadata.is_file() {
        return Err(OpenSearchTlsError::CaNotRegular);
    }
    if metadata.len() > MAX_CA_BYTES {
        return Err(OpenSearchTlsError::CaTooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| OpenSearchTlsError::CaRead)?;
    if bytes.len() as u64 > MAX_CA_BYTES {
        return Err(OpenSearchTlsError::CaTooLarge);
    }
    Ok(bytes)
}

fn validate_bundle(
    bytes: &[u8],
) -> Result<(Vec<u8>, Vec<reqwest::Certificate>), OpenSearchTlsError> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let text = std::str::from_utf8(bytes).map_err(|_| OpenSearchTlsError::InvalidCa)?;
    let mut rest = text.trim_ascii();
    let mut roots = RootCertStore::empty();
    let mut certificates = Vec::new();
    let mut normalized = Vec::new();
    while !rest.is_empty() {
        let body = rest
            .strip_prefix(BEGIN)
            .ok_or(OpenSearchTlsError::InvalidCa)?;
        // Match SDK's line-based delimiters exactly, with LF or CRLF.
        if !body.starts_with('\n') && !body.starts_with("\r\n") {
            return Err(OpenSearchTlsError::InvalidCa);
        }
        let end = rest.find(END).ok_or(OpenSearchTlsError::InvalidCa)?;
        let body = &rest[BEGIN.len()..end];
        if !body.ends_with('\n')
            || !body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=\r\n".contains(&b))
        {
            return Err(OpenSearchTlsError::InvalidCa);
        }
        let end = end + END.len();
        let pem = &rest.as_bytes()[..end];
        let der = CertificateDer::from_pem_slice(pem).map_err(|_| OpenSearchTlsError::InvalidCa)?;
        roots.add(der).map_err(|_| OpenSearchTlsError::InvalidCa)?;
        certificates
            .push(reqwest::Certificate::from_pem(pem).map_err(|_| OpenSearchTlsError::InvalidCa)?);
        // Canonical delimiter lines prevent the SDK from silently dropping a CA
        // when the input has whitespace between otherwise valid PEM blocks.
        normalized.extend_from_slice(pem);
        normalized.push(b'\n');
        rest = rest[end..].trim_ascii();
    }
    if certificates.is_empty() {
        return Err(OpenSearchTlsError::InvalidCa);
    }
    Ok((normalized, certificates))
}

#[cfg(test)]
#[path = "tls_tests.rs"]
mod tests;
