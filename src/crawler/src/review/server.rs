use crate::CrawlerDomainId;
use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::model::{CrawlerReviewPage, SchemaMatrix};
use crate::review::repository::CrawlerReviewRepository;
use crate::review::repository::ReviewRepositoryError;
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::service::{FetchError, HtmlFetcher, ReqwestHtmlFetcher};
use crate::service::crawler_domain_configuration::{
    CrawlerDomainAdministration, CrawlerDomainConfigurationError,
};
use listing_source_core::{Domain, ListingSourceId};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info, warn};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ReviewServerConfigError {
    #[error(transparent)]
    InvalidBindAddress(#[from] std::net::AddrParseError),
    #[error("CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses")]
    AuthenticationRequiredForNonLoopback,
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
}

impl ReviewServerConfig {
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let bind_addr: SocketAddr = std::env::var("CRAWLER_REVIEW_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
            .parse()?;
        let auth_token = std::env::var("CRAWLER_REVIEW_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        Self {
            bind_addr,
            auth_token,
        }
        .validate()
    }

    pub fn validate(self) -> Result<Self, ReviewServerConfigError> {
        if !self.bind_addr.ip().is_loopback() && self.auth_token.is_none() {
            return Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback);
        }
        Ok(self)
    }
}

const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    domain_administration: Arc<dyn CrawlerDomainAdministration>,
    config: ReviewServerConfig,
    html_fetcher: Arc<dyn HtmlFetcher>,
}

impl ReviewServer {
    pub fn new(
        repository: CrawlerReviewRepository,
        domain_administration: Arc<dyn CrawlerDomainAdministration>,
        config: ReviewServerConfig,
    ) -> Self {
        Self {
            repository,
            domain_administration,
            config,
            html_fetcher: Arc::new(ReqwestHtmlFetcher::new()),
        }
    }

    pub fn new_with_fetcher(
        repository: CrawlerReviewRepository,
        domain_administration: Arc<dyn CrawlerDomainAdministration>,
        config: ReviewServerConfig,
        html_fetcher: Arc<dyn HtmlFetcher>,
    ) -> Self {
        Self {
            repository,
            domain_administration,
            config,
            html_fetcher,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(
            bind_addr = %self.config.bind_addr,
            "Crawler review console listening"
        );

        loop {
            let (stream, peer) = listener.accept().await?;
            let server = self.clone();
            tokio::spawn(async move {
                if let Err(err) = server.handle_connection(stream).await {
                    warn!(peer = %peer, error = ?err, "Review console request failed");
                }
            });
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let response = match read_bounded_request(&mut stream).await {
            Ok(Some(request)) => self.route(&request).await,
            Ok(None) => return Ok(()),
            Err(error) => {
                warn!(error = ?error, "Rejected malformed review console request");
                HttpResponse::json(400, &json!({ "error": "bad request" }))
            }
        }
        .to_string();
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    }

    async fn route(&self, request: &str) -> HttpResponse {
        let Some(parsed) = parse_request(request) else {
            return HttpResponse::text(400, "bad request");
        };

        if parsed.path.starts_with("/api/")
            && (!self.authorized(&parsed.headers)
                || (is_mutation_method(&parsed.method) && self.config.auth_token.is_none()))
        {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        }

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/api/listing_sources") => {
                match self.repository.list_listing_sources(200).await {
                    Ok(listing_sources) => HttpResponse::json(200, &listing_sources),
                    Err(err) => internal_error(err),
                }
            }
            ("GET", "/api/reviews") => match self.repository.list_reviews(200).await {
                Ok(reviews) => HttpResponse::json(200, &reviews),
                Err(err) => internal_error(err),
            },
            _ => self.route_dynamic(parsed).await,
        }
    }

    async fn route_dynamic(&self, request: ParsedRequest<'_>) -> HttpResponse {
        if request.method == "GET"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.ends_with("/domains")
        {
            let Some(listing_source_id) =
                parse_listing_source_id_with_suffix(request.path, "/domains")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
            };
            return match self
                .domain_administration
                .list_crawler_domains(listing_source_id)
                .await
            {
                Ok(domains) => HttpResponse::json(
                    200,
                    &domains
                        .into_iter()
                        .map(|domain| {
                            json!({
                                "domain_id": uuid::Uuid::from(domain.domain_id),
                                "listing_source_id": domain.listing_source_id,
                                "domain": domain.domain.as_str(),
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.ends_with("/domains")
        {
            let Some(listing_source_id) =
                parse_listing_source_id_with_suffix(request.path, "/domains")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
            };
            let payload: DomainPayload = match serde_json::from_str(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            let domain = match Domain::try_from(payload.domain) {
                Ok(domain) => domain,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .domain_administration
                .register_crawler_domain(listing_source_id, domain)
                .await
            {
                Ok(domain) => HttpResponse::json(
                    if domain.created { 201 } else { 200 },
                    &json!({
                        "domain_id": uuid::Uuid::from(domain.domain_id),
                        "listing_source_id": domain.listing_source_id,
                        "domain": domain.domain.as_str(),
                        "created": domain.created,
                    }),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "DELETE"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.contains("/domains/")
        {
            let Some((listing_source_id, domain_id)) = parse_listing_source_domain_id(request.path)
            else {
                return HttpResponse::json(
                    400,
                    &json!({ "error": "invalid ListingSource or domain id" }),
                );
            };
            return match self
                .domain_administration
                .remove_crawler_domain(listing_source_id, domain_id)
                .await
            {
                Ok(removal) => HttpResponse::json(
                    200,
                    &json!({
                        "domain_id": uuid::Uuid::from(removal.domain_id),
                        "removed_url_count": removal.removed_url_count,
                        "removed_url_pattern_review_count": removal.removed_url_pattern_review_count,
                    }),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "GET" && request.path == "/api/live-inspect" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }));
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::html(200, &instrument_live_html(url, &html)),
                Err(response) => response,
            };
        }

        if request.method == "GET" && request.path == "/api/live-html" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }));
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::html(200, &html),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/matrix")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/matrix") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let refresh = request
                .query
                .get("refresh")
                .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
            if !refresh {
                match self.cached_schema_matrix(review_id).await {
                    Ok(Some(matrix)) => return HttpResponse::json(200, &matrix),
                    Ok(None) => {}
                    Err(err) => return internal_error(err),
                }
            }
            return match self.refresh_schema_matrix(review_id).await {
                Ok(matrix) => HttpResponse::json(200, &matrix),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/inspect")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/inspect") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((page, html))) => {
                    HttpResponse::html(200, &instrument_review_page(&page, &html))
                }
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/html")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/html") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((_page, html))) => HttpResponse::html(200, &html),
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(response) => response,
            };
        }

        if request.method == "GET" && request.path.starts_with("/api/reviews/") {
            let Some(review_id) = parse_review_id(request.path) else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            return match self.repository.get_review(review_id).await {
                Ok(detail) => HttpResponse::json(200, &detail),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/approve")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/approve") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = parse_action_payload(request.body);
            return match self
                .repository
                .approve_review(review_id, payload.notes.as_deref())
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/reject")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/reject") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = parse_action_payload(request.body);
            return match self
                .repository
                .reject_review(review_id, payload.notes.as_deref(), false)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/needs-repair")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/needs-repair") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = parse_action_payload(request.body);
            return match self
                .repository
                .reject_review(review_id, payload.notes.as_deref(), true)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/schema-field")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/schema-field") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: SchemaFieldPayload = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .update_schema_field(
                    review_id,
                    payload.schema_index,
                    &payload.field,
                    payload.rule,
                )
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::InvalidSchemaField(_))
                | Err(err @ ReviewRepositoryError::RequiredSchemaField(_))
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/candidate")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/candidate") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let candidate_payload: serde_json::Value = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .update_candidate_payload(review_id, candidate_payload)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::InvalidSchemaField(_))
                | Err(err @ ReviewRepositoryError::RequiredSchemaField(_))
                | Err(err @ ReviewRepositoryError::InvalidUrlPatternCandidate)
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err) => internal_error(err),
            };
        }

        HttpResponse::json(404, &json!({ "error": "not found" }))
    }

    fn authorized(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(expected) = self.config.auth_token.as_deref() else {
            return true;
        };
        headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|actual| constant_time_eq(actual.as_bytes(), expected.as_bytes()))
    }

    async fn live_review_pages(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(internal_error)?;
        let mut pages = Vec::with_capacity(detail.pages.len());
        for page in detail.pages {
            let html = self.fetch_live_html(&page).await?;
            pages.push((page, html));
        }
        Ok(pages)
    }

    async fn cached_schema_matrix(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Option<SchemaMatrix>, ReviewRepositoryError> {
        let detail = self.repository.get_review(review_id).await?;
        let Some(value) = detail.review.validation_summary.get("schema_matrix") else {
            return Ok(None);
        };
        match serde_json::from_value::<SchemaMatrix>(value.clone()) {
            Ok(matrix) => Ok(Some(matrix)),
            Err(err) => {
                warn!(
                    review_id = %review_id,
                    error = ?err,
                    "Cached schema matrix is invalid; refreshing from live pages"
                );
                Ok(None)
            }
        }
    }

    async fn refresh_schema_matrix(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<SchemaMatrix, HttpResponse> {
        let pages = self.live_review_pages(review_id).await?;
        let matrix = self
            .repository
            .evaluate_schema_matrix_for_live_pages(review_id, pages)
            .await
            .map_err(internal_error)?;
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(internal_error)?;
        let validation_summary =
            with_cached_schema_matrix(detail.review.validation_summary, &matrix);
        self.repository
            .update_review_validation_summary(review_id, validation_summary)
            .await
            .map_err(internal_error)?;
        Ok(matrix)
    }

    async fn live_review_page(
        &self,
        review_page_id: uuid::Uuid,
    ) -> Result<Option<(CrawlerReviewPage, String)>, HttpResponse> {
        let Some(page) = self
            .repository
            .get_review_page(review_page_id)
            .await
            .map_err(internal_error)?
        else {
            return Ok(None);
        };
        let html = self.fetch_live_html(&page).await?;
        Ok(Some((page, html)))
    }

    async fn fetch_live_html(&self, page: &CrawlerReviewPage) -> Result<String, HttpResponse> {
        let url = Url::parse(&page.url).map_err(|err| {
            HttpResponse::json(
                400,
                &json!({
                    "error": "invalid live review page url",
                    "url": page.url,
                    "details": err.to_string(),
                }),
            )
        })?;
        self.html_fetcher
            .fetch(&url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_fetch_error(page, &err))
    }

    async fn fetch_live_url(&self, raw_url: &str) -> Result<String, HttpResponse> {
        let url = Url::parse(raw_url).map_err(|err| {
            HttpResponse::json(
                400,
                &json!({
                    "error": "invalid live preview url",
                    "url": raw_url,
                    "details": err.to_string(),
                }),
            )
        })?;
        self.html_fetcher
            .fetch(&url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_url_fetch_error(raw_url, &err))
    }
}

async fn read_bounded_request(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut request = Vec::with_capacity(1024);
    let mut read_buffer = [0_u8; 4096];
    let header_end = loop {
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() >= MAX_REQUEST_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers exceed limit",
            ));
        }
        let read = tokio::time::timeout(REQUEST_IO_TIMEOUT, stream.read(&mut read_buffer))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "request read timed out")
            })??;
        if read == 0 {
            return if request.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "incomplete request headers",
                ))
            };
        }
        request.extend_from_slice(&read_buffer[..read]);
    };
    let headers = std::str::from_utf8(&request[..header_end]).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request headers are not UTF-8",
        )
    })?;
    let mut content_length = None;
    for line in headers.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transfer encoding is unsupported",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate content length",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid content length")
            })?;
            if length > MAX_REQUEST_BODY_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request body exceeds limit",
                ));
            }
            content_length = Some(length);
        }
    }
    let total_length = header_end + content_length.unwrap_or(0);
    while request.len() < total_length {
        let read = tokio::time::timeout(REQUEST_IO_TIMEOUT, stream.read(&mut read_buffer))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "request read timed out")
            })??;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete request body",
            ));
        }
        request.extend_from_slice(&read_buffer[..read]);
    }
    String::from_utf8(request[..total_length].to_vec())
        .map(Some)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "request is not UTF-8"))
}

#[derive(Deserialize, Default)]
struct ActionPayload {
    notes: Option<String>,
}

#[derive(Deserialize)]
struct SchemaFieldPayload {
    schema_index: usize,
    field: String,
    rule: Option<ExtractionRule>,
}

#[derive(Deserialize)]
struct DomainPayload {
    domain: String,
}

fn parse_action_payload(body: &str) -> ActionPayload {
    serde_json::from_str(body).unwrap_or_default()
}

fn with_cached_schema_matrix(
    mut validation_summary: serde_json::Value,
    matrix: &SchemaMatrix,
) -> serde_json::Value {
    if let Some(object) = validation_summary.as_object_mut() {
        object.insert("schema_matrix".to_string(), json!(matrix));
        validation_summary
    } else {
        json!({
            "summary": validation_summary,
            "schema_matrix": matrix,
        })
    }
}

fn parse_review_id(path: &str) -> Option<uuid::Uuid> {
    let id = path.strip_prefix("/api/reviews/")?;
    uuid::Uuid::parse_str(id).ok()
}

fn parse_review_id_with_suffix(path: &str, suffix: &str) -> Option<uuid::Uuid> {
    let without_suffix = path.strip_suffix(suffix)?;
    parse_review_id(without_suffix)
}

fn parse_listing_source_id_with_suffix(path: &str, suffix: &str) -> Option<ListingSourceId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/listing-sources/")?;
    uuid::Uuid::parse_str(id).ok().map(Into::into)
}

fn parse_listing_source_domain_id(path: &str) -> Option<(ListingSourceId, CrawlerDomainId)> {
    let rest = path.strip_prefix("/api/listing-sources/")?;
    let (listing_source_id, domain_id) = rest.split_once("/domains/")?;
    Some((
        uuid::Uuid::parse_str(listing_source_id).ok()?.into(),
        uuid::Uuid::parse_str(domain_id).ok()?.into(),
    ))
}

fn parse_page_id_with_suffix(path: &str, suffix: &str) -> Option<uuid::Uuid> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/review-pages/")?;
    uuid::Uuid::parse_str(id).ok()
}

fn domain_configuration_error(error: CrawlerDomainConfigurationError) -> HttpResponse {
    match error {
        CrawlerDomainConfigurationError::ListingSourceNotFound { .. }
        | CrawlerDomainConfigurationError::DomainNotOwnedByListingSource { .. } => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource { .. } => {
            HttpResponse::json(409, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::UnsafeDomain { .. } => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::Database { .. } => internal_error(error),
    }
}

fn internal_error(error: impl std::fmt::Display + std::fmt::Debug) -> HttpResponse {
    error!(error = ?error, "Review API error");
    HttpResponse::json(500, &json!({ "error": "internal server error" }))
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

fn live_fetch_error(page: &CrawlerReviewPage, error: &FetchError) -> HttpResponse {
    warn!(
        review_page_id = %page.review_page_id,
        url = %page.url,
        error = ?error,
        "Failed to fetch live review HTML"
    );
    HttpResponse::json(
        502,
        &json!({
            "error": "failed to fetch live review HTML",
            "url": page.url,
            "details": error.to_string(),
        }),
    )
}

fn live_url_fetch_error(url: &str, error: &FetchError) -> HttpResponse {
    warn!(
        url,
        error = ?error,
        "Failed to fetch live preview HTML"
    );
    HttpResponse::json(
        502,
        &json!({
            "error": "failed to fetch live preview HTML",
            "url": url,
            "details": error.to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::crawler_domain_configuration::{
        CrawlerDomainConfiguration, CrawlerDomainRemoval,
    };

    struct RejectingDomainAdministration;

    #[async_trait::async_trait]
    impl CrawlerDomainAdministration for RejectingDomainAdministration {
        async fn list_crawler_domains(
            &self,
            listing_source_id: ListingSourceId,
        ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }

        async fn register_crawler_domain(
            &self,
            listing_source_id: ListingSourceId,
            _domain: Domain,
        ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }

        async fn remove_crawler_domain(
            &self,
            listing_source_id: ListingSourceId,
            _domain_id: CrawlerDomainId,
        ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }
    }

    async fn read_request_from_socket_chunks(chunks: &[&[u8]]) -> std::io::Result<Option<String>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let reader = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            read_bounded_request(&mut stream).await
        });
        let mut client = TcpStream::connect(address).await?;

        for chunk in chunks {
            client.write_all(chunk).await?;
            tokio::task::yield_now().await;
        }
        client.shutdown().await?;

        reader.await.map_err(std::io::Error::other)?
    }

    async fn review_server_for_test(auth_token: Option<&str>) -> Result<ReviewServer, sqlx::Error> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        Ok(ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: auth_token.map(str::to_owned),
            },
        ))
    }

    #[tokio::test]
    async fn should_accept_fragmented_content_length_body() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = read_request_from_socket_chunks(&[
            b"POST /api/health HTTP/1.1\r\nContent-Length: 15\r\n\r\n{\"key\":",
            b"\"value\"}",
        ])
        .await?;

        assert_eq!(
            request.as_deref(),
            Some("POST /api/health HTTP/1.1\r\nContent-Length: 15\r\n\r\n{\"key\":\"value\"}")
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_request_body_larger_than_limit() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = format!(
            "POST /api/health HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_REQUEST_BODY_BYTES + 1
        );
        let error = read_request_from_socket_chunks(&[request.as_bytes()])
            .await
            .expect_err("oversized request body should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "request body exceeds limit");
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_transfer_encoding_and_duplicate_content_length()
    -> Result<(), Box<dyn std::error::Error>> {
        for (request, expected_error) in [
            (
                "POST /api/health HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                "transfer encoding is unsupported",
            ),
            (
                "POST /api/health HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
                "duplicate content length",
            ),
        ] {
            let error = read_request_from_socket_chunks(&[request.as_bytes()])
                .await
                .expect_err("ambiguous request framing should be rejected");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), expected_error);
        }
        Ok(())
    }

    #[test]
    fn should_cache_schema_matrix_in_validation_summary() {
        let matrix = SchemaMatrix {
            review_id: uuid::Uuid::new_v4(),
            candidates: Vec::new(),
        };

        let summary = with_cached_schema_matrix(json!({ "existing": true }), &matrix);

        assert_eq!(summary["existing"], true);
        let cached = serde_json::from_value::<SchemaMatrix>(summary["schema_matrix"].clone())
            .expect("cached matrix should deserialize");
        assert_eq!(cached.review_id, matrix.review_id);
        assert!(cached.candidates.is_empty());
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
        }
        .validate();

        assert!(matches!(
            result,
            Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback)
        ));
    }

    #[test]
    fn should_allow_non_loopback_review_bind_with_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
        }
        .validate();

        assert!(result.is_ok());
    }

    #[test]
    fn should_allow_loopback_review_bind_without_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: None,
        }
        .validate();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_require_valid_token_for_configured_api_access_and_all_mutations()
    -> Result<(), Box<dyn std::error::Error>> {
        let unauthenticated_server = review_server_for_test(None).await?;
        let unauthenticated_read = unauthenticated_server
            .route("GET /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let unauthenticated_mutation = unauthenticated_server
            .route("POST /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        assert!(unauthenticated_read.starts_with("HTTP/1.1 200"));
        assert!(unauthenticated_mutation.starts_with("HTTP/1.1 401"));

        let authenticated_server = review_server_for_test(Some("review-token")).await?;
        let missing_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let invalid_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\nAuthorization: Bearer wrong-token\r\n\r\n")
            .await
            .to_string();
        let valid_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        let valid_mutation_token = authenticated_server
            .route("POST /api/health HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();

        assert!(missing_token.starts_with("HTTP/1.1 401"));
        assert!(invalid_token.starts_with("HTTP/1.1 401"));
        assert!(valid_token.starts_with("HTTP/1.1 200"));
        assert!(valid_mutation_token.starts_with("HTTP/1.1 404"));
        Ok(())
    }

    #[test]
    fn should_compare_authentication_tokens_without_early_value_match() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"taken"));
        assert!(!constant_time_eq(b"token", b"tokens"));
    }
}
