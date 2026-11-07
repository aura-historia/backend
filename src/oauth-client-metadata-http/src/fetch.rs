use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, USER_AGENT};
use std::net::SocketAddr;
use std::time::Duration;
use url::Url;

const HTTPS_PORT: u16 = 443;
const CLIENT_USER_AGENT: &str = "AuraHistoria-OAuth (client metadata document)";

/// Where a document request goes.
#[derive(Debug, Clone)]
pub(crate) enum Destination {
    /// HTTPS on port 443 to the `client_id` host, connecting only to public addresses.
    PublicHttps,
    /// Serves one host's documents from a plain-HTTP loopback origin; every other host keeps
    /// the public policy. Only acceptance fixtures construct it.
    #[cfg(any(test, feature = "loopback-test-origin"))]
    LoopbackTestOrigin { host: String, origin: SocketAddr },
}

/// A successful document response.
pub(crate) struct FetchedDocument {
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

/// Fetches one document under the outbound policy: no redirects, a `200` JSON response of at
/// most `max_bytes`, and one deadline covering resolution, connection and the body.
pub(crate) async fn fetch_document(
    destination: &Destination,
    client_id: &MetadataDocumentClientId,
    timeout: Duration,
    max_bytes: usize,
    resolver: &Resolver,
) -> Result<FetchedDocument, FetchFailure> {
    tokio::time::timeout(timeout, async {
        let (url, client) = match destination {
            #[cfg(any(test, feature = "loopback-test-origin"))]
            Destination::LoopbackTestOrigin { host, origin } if host == client_id.host() => (
                loopback_url(client_id.url(), *origin)?,
                client_builder(timeout).build()?,
            ),
            _ => {
                let host = client_id.host();
                let addresses = resolver.resolve_public(client_id.url()).await?;
                let client = client_builder(timeout)
                    .https_only(true)
                    .resolve_to_addrs(host, &addresses)
                    .build()?;
                (client_id.url().clone(), client)
            }
        };
        let response = client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, CLIENT_USER_AGENT)
            .send()
            .await?;
        if response.status().is_redirection() {
            return Err(FetchFailure::Redirect);
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(FetchFailure::UnsuccessfulStatus {
                status: response.status().as_u16(),
            });
        }
        if !is_json(response.headers()) {
            return Err(FetchFailure::NotJson);
        }
        let headers = response.headers().clone();
        let body = read_bounded_body(response, max_bytes).await?;
        Ok(FetchedDocument { headers, body })
    })
    .await
    .unwrap_or(Err(FetchFailure::Timeout))
}

fn client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    // No proxy, so connections go only to the pinned addresses; no decompression, so the
    // size limit applies to the bytes received.
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
}

/// Resolves document hosts. Tests can answer chosen hosts without DNS.
#[derive(Debug, Clone, Default)]
pub(crate) struct Resolver {
    #[cfg(test)]
    pub(crate) answers: std::collections::HashMap<String, Vec<SocketAddr>>,
}

impl Resolver {
    /// Resolves the document host and requires every answer to be a public address, so the
    /// pinned connection cannot reach an internal service.
    async fn resolve_public(&self, url: &Url) -> Result<Vec<SocketAddr>, FetchFailure> {
        let host = url.host_str().ok_or(FetchFailure::UnsafeTarget)?;
        if url.scheme() != "https" || url.port_or_known_default() != Some(HTTPS_PORT) {
            return Err(FetchFailure::UnsafeTarget);
        }
        #[cfg(test)]
        if let Some(addresses) = self.answers.get(host) {
            return public_addresses(addresses.clone());
        }
        let addresses = tokio::net::lookup_host((host, HTTPS_PORT))
            .await
            .map_err(FetchFailure::Resolution)?
            .collect::<Vec<_>>();
        public_addresses(addresses)
    }
}

fn public_addresses(addresses: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, FetchFailure> {
    if addresses.is_empty()
        || !addresses
            .iter()
            .all(|address| public_network_policy::is_safe_public_http_destination(address.ip()))
    {
        return Err(FetchFailure::UnsafeTarget);
    }
    Ok(addresses)
}

#[cfg(any(test, feature = "loopback-test-origin"))]
fn loopback_url(document: &Url, origin: SocketAddr) -> Result<Url, FetchFailure> {
    let mut url =
        Url::parse(&format!("http://{origin}")).map_err(|_| FetchFailure::UnsafeTarget)?;
    url.set_path(document.path());
    url.set_query(document.query());
    Ok(url)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, FetchFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(FetchFailure::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > max_bytes.saturating_sub(body.len()) {
            return Err(FetchFailure::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Why a document could not be fetched. Neither message nor sources carry document content.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchFailure {
    #[error("metadata document host does not resolve exclusively to public addresses")]
    UnsafeTarget,
    #[error("metadata document host resolution failed")]
    Resolution(#[source] std::io::Error),
    #[error("metadata document request failed")]
    Request(#[source] reqwest::Error),
    #[error("metadata document fetch timed out")]
    Timeout,
    #[error("metadata document response is a redirect")]
    Redirect,
    #[error("metadata document response status {status} is not 200")]
    UnsuccessfulStatus { status: u16 },
    #[error("metadata document response is not application/json")]
    NotJson,
    #[error("metadata document exceeds the size limit")]
    TooLarge,
}

impl From<reqwest::Error> for FetchFailure {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else {
            Self::Request(error.without_url())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_require_application_json_responses() {
        for content_type in ["application/json", "Application/JSON; charset=utf-8"] {
            let headers = HeaderMap::from_iter([(CONTENT_TYPE, content_type.parse().unwrap())]);
            assert!(is_json(&headers), "{content_type}");
        }
        assert!(!is_json(&HeaderMap::new()));
        for content_type in [
            "text/html",
            "application/jsonp",
            "application/ld+json",
            "text/json",
        ] {
            let headers = HeaderMap::from_iter([(CONTENT_TYPE, content_type.parse().unwrap())]);
            assert!(!is_json(&headers), "{content_type}");
        }
    }

    #[test]
    fn should_admit_only_public_answers() {
        for answers in [
            vec![],
            vec!["127.0.0.1:443"],
            vec!["10.0.0.1:443"],
            vec!["169.254.169.254:443"],
            vec!["[::1]:443"],
            vec!["[fd00::1]:443"],
            vec!["93.184.215.14:443", "192.168.0.1:443"],
        ] {
            let addresses = answers.iter().map(|value| value.parse().unwrap()).collect();
            assert!(
                matches!(public_addresses(addresses), Err(FetchFailure::UnsafeTarget)),
                "{answers:?}"
            );
        }
        assert!(public_addresses(vec!["93.184.215.14:443".parse().unwrap()]).is_ok());
    }

    #[test]
    fn should_map_a_loopback_test_origin_without_changing_path_or_query() {
        let document = Url::parse("https://agent.test/oauth/client.json?v=2").unwrap();
        assert_eq!(
            "http://127.0.0.1:8080/oauth/client.json?v=2",
            loopback_url(&document, "127.0.0.1:8080".parse().unwrap())
                .unwrap()
                .as_str()
        );
    }
}
