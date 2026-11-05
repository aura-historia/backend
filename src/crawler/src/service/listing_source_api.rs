//! Crawler ListingSource scope read from the admin API.
//!
//! The crawler host holds no business database credentials: the sources it may crawl are the
//! ones the admin search endpoint reports with an active `WEB_CRAWL` ingestion method. A refresh
//! applies a complete scope or fails as a whole.

use crate::network::policy::{CrawlerClientError, RequestTimeouts, crawler_client_builder};
use crate::service::listing_source_registration::{
    FallbackCurrency, ListingSourceRegistrationSource, ListingSourceSyncError,
    RegisteredListingSource,
};
use async_trait::async_trait;
use listing_source_core::{
    Domain, ListingIngestionMethod, ListingSourceId, ListingSourceName, ListingSourceSlugId,
};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;
use url::{Host, Url};

pub const API_BASE_URL_ENV: &str = "AURA_API_BASE_URL";
pub const API_TOKEN_ENV: &str = "CRAWLER_API_TOKEN";

const LISTING_SOURCES_PATH: &str = "v1/admin/listing-sources";
const PAGE_SIZE: &str = "100";
/// Loop guard: a scope this large, or a cursor chain this long, is a defect, not data.
const MAX_PAGES: usize = 1000;
const REQUEST_TIMEOUTS: RequestTimeouts = RequestTimeouts {
    connect: Duration::from_secs(10),
    request: Duration::from_secs(30),
};

/// A bearer credential that cannot reach a log through `Debug`, and has no `Display`.
#[derive(Clone)]
pub struct ApiToken(String);

impl ApiToken {
    fn parse(value: &str) -> Result<Self, ListingSourceApiConfigError> {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(ListingSourceApiConfigError::InvalidToken);
        }
        Ok(Self(value.to_owned()))
    }

    fn authorization_header(&self) -> Result<HeaderValue, ListingSourceApiConfigError> {
        let mut header = HeaderValue::from_str(&format!("Bearer {}", self.0))
            .map_err(|_| ListingSourceApiConfigError::InvalidToken)?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiToken(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceApiConfigError {
    #[error("{name} must be set")]
    Missing { name: &'static str },
    #[error("{API_BASE_URL_ENV} is not a valid URL")]
    InvalidBaseUrl,
    #[error("{API_BASE_URL_ENV} must use https; http is accepted only for a loopback host")]
    InsecureBaseUrl,
    #[error("{API_BASE_URL_ENV} must not carry credentials, a query or a fragment")]
    UnexpectedBaseUrlParts,
    #[error("{API_TOKEN_ENV} must be a non-blank bearer token of visible ASCII characters")]
    InvalidToken,
    #[error(transparent)]
    Client(#[from] CrawlerClientError),
}

/// Where the admin API is and the credential to read it with.
#[derive(Debug, Clone)]
pub struct ListingSourceApiConfig {
    endpoint: Url,
    token: ApiToken,
}

impl ListingSourceApiConfig {
    pub fn from_env() -> Result<Self, ListingSourceApiConfigError> {
        Self::new(
            std::env::var(API_BASE_URL_ENV).ok().as_deref(),
            std::env::var(API_TOKEN_ENV).ok().as_deref(),
        )
    }

    pub fn new(
        base_url: Option<&str>,
        token: Option<&str>,
    ) -> Result<Self, ListingSourceApiConfigError> {
        let base_url = base_url.filter(|value| !value.trim().is_empty()).ok_or(
            ListingSourceApiConfigError::Missing {
                name: API_BASE_URL_ENV,
            },
        )?;
        let token = token.filter(|value| !value.trim().is_empty()).ok_or(
            ListingSourceApiConfigError::Missing {
                name: API_TOKEN_ENV,
            },
        )?;
        Ok(Self {
            endpoint: listing_sources_endpoint(base_url.trim())?,
            token: ApiToken::parse(token)?,
        })
    }
}

fn listing_sources_endpoint(base_url: &str) -> Result<Url, ListingSourceApiConfigError> {
    let mut url = Url::parse(base_url).map_err(|_| ListingSourceApiConfigError::InvalidBaseUrl)?;
    let loopback = match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => return Err(ListingSourceApiConfigError::InvalidBaseUrl),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => return Err(ListingSourceApiConfigError::InsecureBaseUrl),
        _ => return Err(ListingSourceApiConfigError::InvalidBaseUrl),
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ListingSourceApiConfigError::UnexpectedBaseUrlParts);
    }
    let path = format!(
        "{}/{LISTING_SOURCES_PATH}",
        url.path().trim_end_matches('/')
    );
    url.set_path(&path);
    Ok(url)
}

/// Reads the `WEB_CRAWL` ListingSources through the admin API, following the cursor to the last
/// page.
pub struct ApiListingSourceRegistrationSource {
    client: reqwest::Client,
    endpoint: Url,
    authorization: HeaderValue,
}

impl ApiListingSourceRegistrationSource {
    pub fn new(config: ListingSourceApiConfig) -> Result<Self, ListingSourceApiConfigError> {
        let client = crawler_client_builder(REQUEST_TIMEOUTS)?
            .build()
            .map_err(CrawlerClientError::Build)?;
        Ok(Self {
            client,
            authorization: config.token.authorization_header()?,
            endpoint: config.endpoint,
        })
    }

    async fn fetch_page(&self, cursor: Option<&str>) -> Result<ListingSourcePage, FetchError> {
        let mut url = self.endpoint.clone();
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("ingestionMethod", ListingIngestionMethod::WebCrawl.as_str())
                .append_pair("size", PAGE_SIZE);
            if let Some(cursor) = cursor {
                query.append_pair("searchAfter", cursor);
            }
        }
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, self.authorization.clone())
            .send()
            .await
            .map_err(|error| FetchError::Transport(error.without_url()))?;
        let status = response.status();
        if !status.is_success() {
            let error_code = response
                .json::<ProblemBody>()
                .await
                .ok()
                .and_then(|problem| problem.error);
            return Err(FetchError::Status { status, error_code });
        }
        response
            .json::<ListingSourcePage>()
            .await
            .map_err(|error| FetchError::Decode(error.without_url()))
    }

    async fn fetch_all(&self) -> Result<Vec<RegisteredListingSource>, FetchError> {
        let mut sources = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut seen_cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page = self.fetch_page(cursor.as_deref()).await?;
            for item in page.items {
                let source = item.into_registered()?;
                if !seen_ids.insert(source.listing_source_id) {
                    return Err(FetchError::DuplicateListingSource(source.listing_source_id));
                }
                sources.push(source);
            }
            match page.search_after {
                None => return Ok(sources),
                Some(next) if next.is_empty() || !seen_cursors.insert(next.clone()) => {
                    return Err(FetchError::CursorDidNotAdvance);
                }
                Some(next) => cursor = Some(next),
            }
        }
        Err(FetchError::TooManyPages)
    }
}

#[async_trait]
impl ListingSourceRegistrationSource for ApiListingSourceRegistrationSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        self.fetch_all()
            .await
            .map_err(|error| ListingSourceSyncError::FetchError(error.to_string()))
    }
}

/// Failure causes of a scope read. Messages never carry the token, a cursor or a response body.
#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("admin API request failed: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("admin API answered {status}{}", error_code_suffix(.error_code))]
    Status {
        status: reqwest::StatusCode,
        error_code: Option<String>,
    },
    #[error("admin API response could not be decoded: {0}")]
    Decode(#[source] reqwest::Error),
    #[error("admin API returned an invalid ListingSource: {0}")]
    InvalidItem(&'static str),
    #[error("admin API returned ListingSource {0} more than once")]
    DuplicateListingSource(ListingSourceId),
    #[error("admin API pagination cursor was empty or repeated")]
    CursorDidNotAdvance,
    #[error("admin API pagination exceeded {MAX_PAGES} pages")]
    TooManyPages,
}

fn error_code_suffix(error_code: &Option<String>) -> String {
    error_code
        .as_deref()
        .filter(|code| {
            code.len() <= 64 && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(|code| format!(" ({code})"))
        .unwrap_or_default()
}

/// Transport shapes stay private to this adapter; unknown fields are ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListingSourcePage {
    items: Vec<ListingSourceItem>,
    search_after: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListingSourceItem {
    listing_source_id: String,
    listing_source_slug_id: String,
    name: String,
    ingestion_methods: Vec<String>,
    #[serde(default)]
    presentation: Option<PresentationItem>,
}

/// The public presentation URLs of a ListingSource; only the site URL is read.
#[derive(Deserialize)]
struct PresentationItem {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct ProblemBody {
    error: Option<String>,
}

/// The host of a shop's public URL as a crawler domain; `None` for anything but an http(s)
/// URL whose host is a domain.
fn public_host(url: &str) -> Option<Domain> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Domain::try_from(url.host_str()?).ok()
}

impl ListingSourceItem {
    fn into_registered(self) -> Result<RegisteredListingSource, FetchError> {
        if !self
            .ingestion_methods
            .iter()
            .any(|method| method == ListingIngestionMethod::WebCrawl.as_str())
        {
            return Err(FetchError::InvalidItem(
                "no active WEB_CRAWL ingestion method",
            ));
        }
        Ok(RegisteredListingSource {
            listing_source_id: self
                .listing_source_id
                .parse()
                .map_err(|_| FetchError::InvalidItem("listingSourceId"))?,
            listing_source_name: ListingSourceName::try_from(self.name)
                .map_err(|_| FetchError::InvalidItem("name"))?,
            listing_source_slug: ListingSourceSlugId::raw(&self.listing_source_slug_id)
                .map_err(|_| FetchError::InvalidItem("listingSourceSlugId"))?,
            crawl_enabled: true,
            fallback_currency: FallbackCurrency::NotSupplied,
            public_host: self
                .presentation
                .and_then(|presentation| presentation.url)
                .and_then(|url| public_host(&url)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const TOKEN: &str = "secret-crawler-token-4711";

    /// What the stub saw of one request.
    #[derive(Debug, Clone)]
    struct SeenRequest {
        target: String,
        authorization: Option<String>,
    }

    /// Serves the scripted `(status, body)` answers in order on a loopback port and records the
    /// requests. Every request after the script gets a 500.
    struct StubApi {
        base_url: String,
        seen: Arc<Mutex<Vec<SeenRequest>>>,
    }

    impl StubApi {
        async fn serve(script: Vec<(u16, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
            let base_url = format!("http://{}", listener.local_addr().expect("stub address"));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut script = script.into_iter();
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        return;
                    };
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&buffer).into_owned();
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or_default()
                        .to_owned();
                    let authorization = request.lines().find_map(|line| {
                        line.strip_prefix("authorization: ")
                            .or_else(|| line.strip_prefix("Authorization: "))
                            .map(str::to_owned)
                    });
                    recorded
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(SeenRequest {
                            target,
                            authorization,
                        });
                    let (status, body) = script
                        .next()
                        .unwrap_or((500, r#"{"error":"UNSCRIPTED"}"#.to_owned()));
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            Self { base_url, seen }
        }

        fn source(&self) -> ApiListingSourceRegistrationSource {
            let config = ListingSourceApiConfig::new(Some(&self.base_url), Some(TOKEN))
                .expect("loopback stub config");
            ApiListingSourceRegistrationSource::new(config).expect("source")
        }

        fn requests(&self) -> Vec<SeenRequest> {
            self.seen
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    fn item(id: &ListingSourceId, slug: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "listingSourceId": id.to_string(),
            "listingSourceSlugId": slug,
            "name": name,
            "ingestionMethods": ["WEB_CRAWL", "SHOPIFY"],
            "operator": {"unknown": "fields are ignored"},
        })
    }

    fn page(items: Vec<serde_json::Value>, search_after: Option<&str>) -> (u16, String) {
        let mut body = serde_json::json!({"items": items, "size": 0});
        if let Some(cursor) = search_after {
            body["searchAfter"] = cursor.into();
        }
        (200, body.to_string())
    }

    async fn fetch(stub: &StubApi) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        stub.source().fetch_registered_listing_sources().await
    }

    fn failure_message(
        result: Result<Vec<RegisteredListingSource>, ListingSourceSyncError>,
    ) -> String {
        match result {
            Err(ListingSourceSyncError::FetchError(message)) => message,
            other => panic!("expected a fetch error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn should_map_the_host_of_the_presentation_url_as_the_public_host() {
        let ids: Vec<ListingSourceId> = (0..4).map(|_| ListingSourceId::new()).collect();
        let presentations = [
            Some("https://www.hartantiques.com/"),
            Some("https://www.stamplove.co.kr/main/main.php"),
            Some("mailto:shop@example.test"),
            None,
        ];
        let items = ids
            .iter()
            .zip(presentations)
            .map(|(id, url)| {
                let mut item = item(id, "shop", "Shop");
                if let Some(url) = url {
                    item["presentation"] = serde_json::json!({ "url": url });
                }
                item
            })
            .collect();
        let stub = StubApi::serve(vec![page(items, None)]).await;

        let sources = fetch(&stub).await.expect("complete scope");

        assert_eq!(
            sources
                .iter()
                .map(|source| source.public_host.as_ref().map(ToString::to_string))
                .collect::<Vec<_>>(),
            vec![
                Some("www.hartantiques.com".to_owned()),
                Some("www.stamplove.co.kr".to_owned()),
                None,
                None,
            ]
        );
    }

    #[tokio::test]
    async fn should_follow_the_cursor_across_pages_and_map_every_source_as_crawl_enabled() {
        let (first, second, third) = (
            ListingSourceId::new(),
            ListingSourceId::new(),
            ListingSourceId::new(),
        );
        let stub = StubApi::serve(vec![
            page(
                vec![item(&first, "first-shop", "First Shop")],
                Some("cursor 1/="),
            ),
            page(
                vec![
                    item(&second, "second-shop", "Second Shop"),
                    item(&third, "third-shop", "Third Shop"),
                ],
                None,
            ),
        ])
        .await;

        let sources = fetch(&stub).await.expect("complete scope");

        assert_eq!(
            vec![first, second, third],
            sources
                .iter()
                .map(|source| source.listing_source_id)
                .collect::<Vec<_>>()
        );
        assert!(sources.iter().all(|source| source.crawl_enabled));
        assert!(
            sources
                .iter()
                .all(|source| source.fallback_currency == FallbackCurrency::NotSupplied)
        );
        assert_eq!("Second Shop", sources[1].listing_source_name.as_ref());
        assert_eq!("second-shop", sources[1].listing_source_slug.to_string());
        let requests = stub.requests();
        assert_eq!(2, requests.len());
        assert!(requests[0].target.starts_with("/v1/admin/listing-sources?"));
        assert!(requests[0].target.contains("ingestionMethod=WEB_CRAWL"));
        assert!(requests[0].target.contains("size=100"));
        assert!(!requests[0].target.contains("searchAfter"));
        assert!(requests[1].target.contains("searchAfter=cursor+1%2F%3D"));
        for request in &requests {
            assert_eq!(
                Some(format!("Bearer {TOKEN}")),
                request.authorization,
                "every page carries the bearer token"
            );
        }
    }

    #[tokio::test]
    async fn should_return_an_empty_scope_for_a_single_empty_terminal_page() {
        let stub = StubApi::serve(vec![page(vec![], None)]).await;

        assert!(fetch(&stub).await.expect("empty scope").is_empty());
        assert_eq!(1, stub.requests().len());
    }

    #[tokio::test]
    async fn should_fail_the_whole_refresh_when_a_later_page_is_not_successful() {
        let first = ListingSourceId::new();
        let stub = StubApi::serve(vec![
            page(vec![item(&first, "first-shop", "First Shop")], Some("next")),
            (403, r#"{"status":403,"title":"Forbidden","error":"FORBIDDEN","detail":"private body text"}"#.to_owned()),
        ])
        .await;

        let message = failure_message(fetch(&stub).await);

        assert!(message.contains("403"), "{message}");
        assert!(message.contains("FORBIDDEN"), "{message}");
        assert!(!message.contains("private body text"), "{message}");
        assert!(
            !message.contains("next"),
            "the cursor stays out of errors: {message}"
        );
        assert!(!message.contains(TOKEN));
    }

    #[tokio::test]
    async fn should_fail_when_the_response_is_not_the_expected_json() {
        let stub = StubApi::serve(vec![(200, "<html>not json</html>".to_owned())]).await;

        let message = failure_message(fetch(&stub).await);

        assert!(message.contains("could not be decoded"), "{message}");
    }

    #[tokio::test]
    async fn should_fail_when_the_server_is_unreachable_without_leaking_the_token() {
        let stub = StubApi::serve(vec![]).await;
        let source = stub.source();
        let unreachable = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = unreachable.local_addr().expect("address");
        drop(unreachable);
        let config = ListingSourceApiConfig::new(Some(&format!("http://{address}")), Some(TOKEN))
            .expect("config");
        let source_without_server =
            ApiListingSourceRegistrationSource::new(config).expect("source");
        drop(source);

        let message = failure_message(
            source_without_server
                .fetch_registered_listing_sources()
                .await,
        );

        assert!(message.contains("request failed"), "{message}");
        assert!(!message.contains(TOKEN));
    }

    #[tokio::test]
    async fn should_fail_closed_on_an_invalid_item_without_applying_a_partial_scope() {
        let valid = ListingSourceId::new();
        let invalid_values = [
            serde_json::json!({"listingSourceId": "not-an-id", "listingSourceSlugId": "shop", "name": "Shop", "ingestionMethods": ["WEB_CRAWL"]}),
            serde_json::json!({"listingSourceId": valid.to_string(), "listingSourceSlugId": "Bad Slug", "name": "Shop", "ingestionMethods": ["WEB_CRAWL"]}),
            serde_json::json!({"listingSourceId": valid.to_string(), "listingSourceSlugId": "shop", "name": "   ", "ingestionMethods": ["WEB_CRAWL"]}),
            serde_json::json!({"listingSourceId": valid.to_string(), "listingSourceSlugId": "shop", "name": "Shop", "ingestionMethods": ["SHOPIFY"]}),
            serde_json::json!({"listingSourceId": valid.to_string(), "name": "Shop", "ingestionMethods": ["WEB_CRAWL"]}),
        ];
        for invalid in invalid_values {
            let stub = StubApi::serve(vec![page(
                vec![
                    item(&ListingSourceId::new(), "good-shop", "Good Shop"),
                    invalid.clone(),
                ],
                None,
            )])
            .await;

            let result = fetch(&stub).await;

            assert!(
                matches!(result, Err(ListingSourceSyncError::FetchError(_))),
                "{invalid} must fail the refresh, got {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_fail_when_a_source_repeats_across_pages() {
        let repeated = ListingSourceId::new();
        let stub = StubApi::serve(vec![
            page(vec![item(&repeated, "shop", "Shop")], Some("next")),
            page(vec![item(&repeated, "shop", "Shop")], None),
        ])
        .await;

        assert!(failure_message(fetch(&stub).await).contains("more than once"));
    }

    #[tokio::test]
    async fn should_fail_when_a_cursor_repeats() {
        let stub = StubApi::serve(vec![
            page(
                vec![item(&ListingSourceId::new(), "one", "One")],
                Some("same"),
            ),
            page(
                vec![item(&ListingSourceId::new(), "two", "Two")],
                Some("same"),
            ),
        ])
        .await;

        assert!(failure_message(fetch(&stub).await).contains("cursor"));
        assert_eq!(2, stub.requests().len());
    }

    #[test]
    fn should_redact_the_token_from_debug_output() {
        let config = ListingSourceApiConfig::new(Some("https://api.example.test"), Some(TOKEN))
            .expect("config");

        assert!(!format!("{config:?}").contains(TOKEN));
        assert!(!format!("{:?}", config.token).contains(TOKEN));
    }

    #[test]
    fn should_build_the_endpoint_from_the_base_url() {
        for (base, endpoint) in [
            (
                "https://api.example.test",
                "https://api.example.test/v1/admin/listing-sources",
            ),
            (
                "https://api.example.test/",
                "https://api.example.test/v1/admin/listing-sources",
            ),
            (
                "https://api.example.test/dev/",
                "https://api.example.test/dev/v1/admin/listing-sources",
            ),
            (
                "http://localhost:3000",
                "http://localhost:3000/v1/admin/listing-sources",
            ),
            (
                "http://127.0.0.1:3000",
                "http://127.0.0.1:3000/v1/admin/listing-sources",
            ),
            (
                "http://[::1]:3000",
                "http://[::1]:3000/v1/admin/listing-sources",
            ),
        ] {
            let config = ListingSourceApiConfig::new(Some(base), Some(TOKEN)).expect(base);
            assert_eq!(endpoint, config.endpoint.as_str(), "{base}");
        }
    }

    #[test]
    fn should_reject_unsafe_or_incomplete_api_configuration() {
        let rejected = |base: Option<&str>, token: Option<&str>| {
            ListingSourceApiConfig::new(base, token)
                .expect_err("configuration must be rejected")
                .to_string()
        };

        assert!(rejected(None, Some(TOKEN)).contains(API_BASE_URL_ENV));
        assert!(rejected(Some("  "), Some(TOKEN)).contains(API_BASE_URL_ENV));
        assert!(rejected(Some("https://api.example.test"), None).contains(API_TOKEN_ENV));
        assert!(rejected(Some("https://api.example.test"), Some(" ")).contains(API_TOKEN_ENV));
        assert!(rejected(Some("http://api.example.test"), Some(TOKEN)).contains("https"));
        assert!(rejected(Some("ftp://api.example.test"), Some(TOKEN)).contains("not a valid URL"));
        assert!(rejected(Some("not a url"), Some(TOKEN)).contains("not a valid URL"));
        assert!(
            rejected(Some("https://user:pw@api.example.test"), Some(TOKEN)).contains("credentials")
        );
        assert!(rejected(Some("https://api.example.test/?a=b"), Some(TOKEN)).contains("query"));
        assert!(
            rejected(Some("https://api.example.test"), Some("two words")).contains(API_TOKEN_ENV)
        );
        for message in [
            rejected(Some("https://api.example.test"), Some("bad\u{7f}token")),
            rejected(Some("http://api.example.test"), Some(TOKEN)),
        ] {
            assert!(!message.contains(TOKEN));
        }
    }
}
