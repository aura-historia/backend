//! Transport settings that every OpenSearch client composition shares.

use opensearch::cert::{Certificate, CertificateValidation};
use opensearch::http::transport::TransportBuilder;

/// PEM certificates of the private CA that OpenSearch's TLS certificate chains to. They are
/// trusted in addition to the public web roots; unset trusts only the public roots. On the host
/// the certificate comes from a file through `OPENSEARCH_CA_CERT_FILE`.
pub const CA_CERTIFICATE_ENV: &str = "OPENSEARCH_CA_CERT";

/// The configured private CA holds no parsable PEM certificate.
#[derive(Debug, thiserror::Error)]
#[error("invalid {CA_CERTIFICATE_ENV}: expected PEM certificates")]
pub struct InvalidCaCertificate {
    #[source]
    source: opensearch::Error,
}

/// Adds the configured private CA, if any, to the client's trusted roots. Hostname verification
/// stays on, so the node certificate must name the host the endpoint URL uses.
pub fn trust_private_ca(
    builder: TransportBuilder,
    pem: Option<&str>,
) -> Result<TransportBuilder, InvalidCaCertificate> {
    let Some(pem) = pem else {
        return Ok(builder);
    };
    let certificate =
        Certificate::from_pem(pem.as_bytes()).map_err(|source| InvalidCaCertificate { source })?;
    Ok(builder.cert_validation(CertificateValidation::Full(certificate)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opensearch::http::transport::SingleNodeConnectionPool;

    const TEST_CA: &str = "-----BEGIN CERTIFICATE-----
MIIBojCCAUmgAwIBAgIUQXseohzVRudDnMmBC3cuhkGJ5HgwCgYIKoZIzj0EAwIw
JjEkMCIGA1UEAwwbcGxhdGZvcm0tb3BlbnNlYXJjaC10ZXN0LWNhMCAXDTI2MTEx
ODE0MTMyN1oYDzIxMjYxMDI1MTQxMzI3WjAmMSQwIgYDVQQDDBtwbGF0Zm9ybS1v
cGVuc2VhcmNoLXRlc3QtY2EwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAARzVicC
8wHdCpRnzt20EFk6isoEEu6AUmZ3tfLvgDQPVrNV8D701eYPymyC+TKQQ7gD9VDo
iT8KLlSdxJNY9Z1ko1MwUTAdBgNVHQ4EFgQU5WhSLq5ltOwxTgXQztxkt0iNs00w
HwYDVR0jBBgwFoAU5WhSLq5ltOwxTgXQztxkt0iNs00wDwYDVR0TAQH/BAUwAwEB
/zAKBggqhkjOPQQDAgNHADBEAiBFVggldqhoB1WLH+CdTCtW6BGH6C48JWgSvybv
R9iiuwIgUci2StLl0Q0MGaeJauB4iIiooDLr3w1Ghqw5rf49aAs=
-----END CERTIFICATE-----
";

    fn builder() -> TransportBuilder {
        TransportBuilder::new(SingleNodeConnectionPool::new(
            "https://opensearch:9200".parse().unwrap(),
        ))
    }

    #[test]
    fn without_a_private_ca_the_client_builds_with_public_roots_only() {
        trust_private_ca(builder(), None).unwrap().build().unwrap();
    }

    #[test]
    fn a_private_ca_is_added_to_the_trusted_roots() {
        trust_private_ca(builder(), Some(TEST_CA))
            .unwrap()
            .build()
            .unwrap();
    }

    #[test]
    fn a_setting_without_a_certificate_fails_closed() {
        for pem in [
            "",
            "not a certificate",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n",
        ] {
            let Err(error) = trust_private_ca(builder(), Some(pem)) else {
                panic!("a setting without a certificate was accepted");
            };
            assert_eq!(
                "invalid OPENSEARCH_CA_CERT: expected PEM certificates",
                error.to_string()
            );
        }
    }
}
