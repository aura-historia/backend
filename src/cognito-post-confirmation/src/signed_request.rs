//! Signed requests from a Cognito trigger to the host's internal endpoints. The triggers reach
//! the host only through the public API hostname, so an endpoint must be HTTPS (plain HTTP only
//! on a loopback host, for tests), and redirects are never followed.
use crate::signature::{SIGNATURE_HEADER, SignatureError, SigningKey, TIMESTAMP_HEADER, sign};
use std::time::Duration;
use url::{Host, Url};

pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// Safe to display: no configuration value is retained.
#[derive(Debug, thiserror::Error)]
pub enum SignedHostClientConfigError {
    #[error("the host endpoint must be an HTTPS URL, or HTTP on a loopback host")]
    InvalidEndpoint,
    #[error("failed to build the HTTP client")]
    Client(#[source] reqwest::Error),
}

/// Why a signed request did not succeed. Never carries the request body.
#[derive(Debug, thiserror::Error)]
pub enum SignedRequestError {
    #[error("failed to sign the request")]
    Sign(#[source] SignatureError),
    #[error("the request to the host endpoint failed or timed out")]
    Transport(#[source] reqwest::Error),
    #[error("the host endpoint refused the request with status {status}")]
    Refused { status: u16 },
}

/// Posts signed, timestamped JSON bodies to one host endpoint.
pub struct SignedHostClient {
    client: reqwest::Client,
    endpoint: Url,
    key: SigningKey,
}

impl SignedHostClient {
    /// `timeout` bounds each request, from connecting until its response body is read.
    pub fn new(
        endpoint: &str,
        key: SigningKey,
        timeout: Duration,
    ) -> Result<Self, SignedHostClientConfigError> {
        let endpoint =
            trusted_endpoint(endpoint).ok_or(SignedHostClientConfigError::InvalidEndpoint)?;
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(SignedHostClientConfigError::Client)?;
        Ok(Self {
            client,
            endpoint,
            key,
        })
    }

    /// Posts `body` signed at `now` (Unix seconds) and returns the response only when it is 2xx.
    /// `correlation_id` (the invocation's request ID) ties the host's logs to the invocation.
    pub async fn post_json(
        &self,
        body: Vec<u8>,
        now: i64,
        correlation_id: &str,
    ) -> Result<reqwest::Response, SignedRequestError> {
        let signature = sign(&self.key, now, &body).map_err(SignedRequestError::Sign)?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(TIMESTAMP_HEADER, now.to_string())
            .header(SIGNATURE_HEADER, signature)
            .header(CORRELATION_ID_HEADER, correlation_id)
            .body(body)
            .send()
            .await
            .map_err(|error| SignedRequestError::Transport(error.without_url()))?;
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(SignedRequestError::Refused {
                status: status.as_u16(),
            })
        }
    }
}

fn trusted_endpoint(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let loopback = match url.host()? {
        Host::Domain(domain) => domain == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    };
    let trusted_scheme = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    (trusted_scheme
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_only_https_or_loopback_http_endpoints() {
        for endpoint in [
            "https://api.aura-historia.com/internal/cognito/post-confirmation",
            "http://127.0.0.1:8080/internal/cognito/post-confirmation",
            "http://localhost:8080/internal/cognito/post-confirmation",
        ] {
            assert!(trusted_endpoint(endpoint).is_some(), "{endpoint}");
        }
        for endpoint in [
            "",
            "api.aura-historia.com/internal",
            "http://api.aura-historia.com/internal/cognito/post-confirmation",
            "https://user:secret@api.aura-historia.com/internal",
            "https://api.aura-historia.com/internal?next=x",
            "https://api.aura-historia.com/internal#x",
        ] {
            assert!(trusted_endpoint(endpoint).is_none(), "{endpoint}");
        }
    }
}
