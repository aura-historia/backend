//! Fetches OAuth Client ID Metadata Documents (draft-ietf-oauth-client-id-metadata-document)
//! under a fail-closed outbound policy and caches them within fixed lifetime bounds.
//! See `docs/oauth.md`.

mod cache;
mod document;
mod fetch;

use application::error::box_error;
use cache::{DocumentCache, LifetimeBounds};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use fetch::{Destination, FetchFailure, Resolver, fetch_document};
use oauth_core::metadata_document_client::ClientMetadataDocumentClaims;
use oauth_service::ports::{ClientMetadataDocumentLoadError, ClientMetadataDocumentSource};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Largest accepted document; metadata documents are small JSON objects.
pub const MAX_DOCUMENT_BYTES: usize = 5 * 1024;
/// Deadline for resolving, connecting and reading one document.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
/// Shortest reuse of a fetched document, whatever its cache headers say.
pub const MIN_CACHE_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// Longest reuse of a fetched document; after it a failed fetch fails the request.
pub const MAX_CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);
const CACHE_CAPACITY: usize = 1024;

/// Loads client metadata documents over HTTPS and caches them per process.
///
/// Only HTTPS on port 443 to hosts that resolve exclusively to public addresses is requested.
/// Redirects, non-`200` responses, other content types than `application/json`, documents
/// above [`MAX_DOCUMENT_BYTES`] and fetches exceeding [`FETCH_TIMEOUT`] fail. A document is
/// reused for its `Cache-Control`/`Expires` lifetime, bounded by [`MIN_CACHE_LIFETIME`] and
/// [`MAX_CACHE_LIFETIME`], and never after it.
#[derive(Clone)]
pub struct HttpClientMetadataDocumentSource {
    inner: Arc<Inner>,
}

struct Inner {
    destination: Destination,
    timeout: Duration,
    max_bytes: usize,
    lifetime: LifetimeBounds,
    cache: DocumentCache,
    resolver: Resolver,
}

impl HttpClientMetadataDocumentSource {
    pub fn new() -> Self {
        Self::with(Destination::PublicHttps, FETCH_TIMEOUT)
    }

    /// Serves `host`'s documents over plain HTTP from `origin` on loopback, with `timeout`,
    /// so a local acceptance fixture can act as a client's document host. Every other host
    /// keeps the public policy.
    #[cfg(any(test, feature = "loopback-test-origin"))]
    pub fn with_loopback_test_origin(
        host: impl Into<String>,
        origin: std::net::SocketAddr,
        timeout: Duration,
    ) -> Self {
        Self::with(
            Destination::LoopbackTestOrigin {
                host: host.into(),
                origin,
            },
            timeout,
        )
    }

    fn with(destination: Destination, timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                destination,
                timeout,
                max_bytes: MAX_DOCUMENT_BYTES,
                lifetime: LifetimeBounds {
                    min: MIN_CACHE_LIFETIME,
                    max: MAX_CACHE_LIFETIME,
                },
                cache: DocumentCache::new(CACHE_CAPACITY),
                resolver: Resolver::default(),
            }),
        }
    }
}

impl Default for HttpClientMetadataDocumentSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ClientMetadataDocumentSource for HttpClientMetadataDocumentSource {
    async fn load(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<ClientMetadataDocumentClaims, ClientMetadataDocumentLoadError> {
        let inner = &self.inner;
        if let Some(claims) = inner.cache.get(client_id.as_str(), Instant::now()) {
            return Ok(claims);
        }
        let fetched = fetch_document(
            &inner.destination,
            client_id,
            inner.timeout,
            inner.max_bytes,
            &inner.resolver,
        )
        .await
        .map_err(|failure| match failure {
            FetchFailure::NotJson => ClientMetadataDocumentLoadError::Malformed {
                source: box_error(failure),
            },
            failure => ClientMetadataDocumentLoadError::Unavailable {
                source: box_error(failure),
            },
        })?;
        let claims = document::parse_claims(&fetched.body).map_err(|source| {
            ClientMetadataDocumentLoadError::Malformed {
                source: box_error(source),
            }
        })?;
        let lifetime = inner.lifetime.lifetime(&fetched.headers, SystemTime::now());
        let now = Instant::now();
        inner
            .cache
            .insert(client_id.as_str(), claims.clone(), now + lifetime, now);
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const HOST: &str = "agent.test";
    const VALID: &str = r#"{"client_id":"https://agent.test/client.json","client_name":"Agent"}"#;

    /// A loopback HTTP/1.1 origin that answers every request with the current canned response.
    struct Origin {
        address: SocketAddr,
        requests: Arc<AtomicUsize>,
        response: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl Origin {
        async fn serving(response: Option<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let response = Arc::new(Mutex::new(response));
            let (counter, canned) = (requests.clone(), response.clone());
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let (counter, canned) = (counter.clone(), canned.clone());
                    tokio::spawn(async move {
                        let mut request = Vec::new();
                        let mut buffer = [0; 1024];
                        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                            match stream.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(read) => request.extend_from_slice(&buffer[..read]),
                            }
                        }
                        counter.fetch_add(1, Ordering::SeqCst);
                        let response = canned.lock().unwrap().clone();
                        match response {
                            Some(response) => {
                                let _ = stream.write_all(&response).await;
                                let _ = stream.shutdown().await;
                            }
                            // Never answer, holding the connection open.
                            None => tokio::time::sleep(Duration::from_secs(30)).await,
                        }
                    });
                }
            });
            Self {
                address,
                requests,
                response,
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }

        fn respond(&self, response: Vec<u8>) {
            *self.response.lock().unwrap() = Some(response);
        }

        fn source(&self) -> HttpClientMetadataDocumentSource {
            HttpClientMetadataDocumentSource::with_loopback_test_origin(
                HOST,
                self.address,
                Duration::from_millis(500),
            )
        }
    }

    fn response(status: &str, headers: &[&str], body: &str, content_length: bool) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for header in headers {
            response.push_str(header);
            response.push_str("\r\n");
        }
        if content_length {
            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        response.push_str("\r\n");
        response.push_str(body);
        response.into_bytes()
    }

    fn json(body: &str) -> Vec<u8> {
        response("200 OK", &["Content-Type: application/json"], body, true)
    }

    fn client_id(value: &str) -> MetadataDocumentClientId {
        MetadataDocumentClientId::try_from(value).unwrap()
    }

    fn document() -> MetadataDocumentClientId {
        client_id("https://agent.test/client.json")
    }

    fn failure(error: ClientMetadataDocumentLoadError) -> (&'static str, FetchFailure) {
        let (kind, source) = match error {
            ClientMetadataDocumentLoadError::Unavailable { source } => ("unavailable", source),
            ClientMetadataDocumentLoadError::Malformed { source } => ("malformed", source),
        };
        let failure = source
            .downcast::<FetchFailure>()
            .map(|failure| *failure)
            .unwrap_or_else(|source| panic!("unexpected failure: {source}"));
        (kind, failure)
    }

    #[tokio::test]
    async fn should_load_a_document_and_reuse_it_within_its_lifetime() {
        let origin = Origin::serving(Some(json(VALID))).await;
        let source = origin.source();

        let claims = source.load(&document()).await.unwrap();
        assert_eq!(Some("Agent"), claims.client_name.as_deref());
        assert_eq!(claims, source.load(&document()).await.unwrap());
        assert_eq!(1, origin.requests());
    }

    #[tokio::test]
    async fn should_refetch_after_the_lifetime_and_never_serve_a_stale_document() {
        let origin = Origin::serving(Some(json(VALID))).await;
        let mut source = origin.source();
        Arc::get_mut(&mut source.inner).unwrap().lifetime = LifetimeBounds {
            min: Duration::from_millis(50),
            max: Duration::from_millis(50),
        };

        source.load(&document()).await.unwrap();
        origin.respond(response("503 Service Unavailable", &[], "", true));
        tokio::time::sleep(Duration::from_millis(80)).await;
        let error = source.load(&document()).await.unwrap_err();

        assert!(matches!(
            failure(error),
            (
                "unavailable",
                FetchFailure::UnsuccessfulStatus { status: 503 }
            )
        ));
        assert_eq!(2, origin.requests());
    }

    #[tokio::test]
    async fn should_refuse_redirects_without_following_them() {
        let origin = Origin::serving(Some(response(
            "302 Found",
            &["Location: https://agent.test/other.json"],
            "",
            true,
        )))
        .await;

        let error = origin.source().load(&document()).await.unwrap_err();

        assert!(matches!(
            failure(error),
            ("unavailable", FetchFailure::Redirect)
        ));
        assert_eq!(1, origin.requests());
    }

    #[tokio::test]
    async fn should_refuse_documents_above_the_size_limit() {
        let oversized = format!(
            r#"{{"client_id":"https://agent.test/client.json","padding":"{}"}}"#,
            "x".repeat(MAX_DOCUMENT_BYTES)
        );
        for content_length in [true, false] {
            let origin = Origin::serving(Some(response(
                "200 OK",
                &["Content-Type: application/json"],
                &oversized,
                content_length,
            )))
            .await;

            let error = origin.source().load(&document()).await.unwrap_err();

            assert!(matches!(
                failure(error),
                ("unavailable", FetchFailure::TooLarge)
            ));
        }
    }

    #[tokio::test]
    async fn should_refuse_responses_that_are_not_successful_json() {
        for (response, expected) in [
            (
                response("200 OK", &["Content-Type: text/html"], VALID, true),
                "malformed",
            ),
            (response("200 OK", &[], VALID, true), "malformed"),
            (response("404 Not Found", &[], "", true), "unavailable"),
            (
                response(
                    "204 No Content",
                    &["Content-Type: application/json"],
                    "",
                    true,
                ),
                "unavailable",
            ),
        ] {
            let origin = Origin::serving(Some(response)).await;
            let error = origin.source().load(&document()).await.unwrap_err();
            assert_eq!(expected, failure(error).0);
        }

        let origin = Origin::serving(Some(json("[]"))).await;
        let error = origin.source().load(&document()).await.unwrap_err();
        assert!(matches!(
            error,
            ClientMetadataDocumentLoadError::Malformed { .. }
        ));
    }

    #[tokio::test]
    async fn should_fail_a_fetch_that_exceeds_the_deadline() {
        let origin = Origin::serving(None).await;
        let started = Instant::now();

        let error = origin.source().load(&document()).await.unwrap_err();

        assert!(matches!(
            failure(error),
            ("unavailable", FetchFailure::Timeout)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn should_refuse_hosts_that_resolve_to_non_public_addresses_before_connecting() {
        let origin = Origin::serving(Some(json(VALID))).await;
        let mut source = origin.source();
        Arc::get_mut(&mut source.inner).unwrap().resolver.answers = [
            (
                "private.example".to_owned(),
                vec!["10.0.0.1:443".parse().unwrap()],
            ),
            ("loopback.example".to_owned(), vec![origin.address]),
            (
                "mixed.example".to_owned(),
                vec![
                    "93.184.215.14:443".parse().unwrap(),
                    "[::1]:443".parse().unwrap(),
                ],
            ),
        ]
        .into();

        for value in [
            "https://private.example/client.json",
            "https://loopback.example/client.json",
            "https://mixed.example/client.json",
        ] {
            let error = source.load(&client_id(value)).await.unwrap_err();
            assert!(
                matches!(failure(error), ("unavailable", FetchFailure::UnsafeTarget)),
                "{value}"
            );
        }
        assert_eq!(0, origin.requests());
    }
}
