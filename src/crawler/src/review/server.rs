use crate::network::policy::NetworkErrorKind;
use crate::network::robots::MAX_CRAWL_DELAY;
use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::matrix_refresh::{MatrixRefreshOutcome, MatrixRefreshStatus, MatrixRefreshes};
use crate::review::model::{CrawlerReviewPage, SchemaMatrix};
use crate::review::repository::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::schema_evaluation::evaluate_cross_page_findings;
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
use crate::service::crawler_domain_configuration::{
    CrawlerDomainAdministration, CrawlerDomainConfigurationError,
};
use crate::{CrawlerDomainId, CrawlerReviewId, CrawlerReviewPageId};
use dashmap::DashMap;
use listing_source_core::{Domain, ListingSourceId};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ReviewServerConfigError {
    #[error("CRAWLER_REVIEW_BIND_ADDR must be host:port")]
    InvalidBindAddress(#[from] std::net::AddrParseError),
    #[error("CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses")]
    AuthenticationRequiredForNonLoopback,
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    /// Lets same-origin browser requests mutate review state without a token on a
    /// loopback bind. Only the local `dev` binary opts in through
    /// [`ReviewServerConfig::allow_local_mutations_without_token`], which keeps it
    /// limited to a loopback bind without a token.
    local_mutations_without_token: bool,
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
            local_mutations_without_token: false,
        }
        .validate()
    }

    /// Enables tokenless mutations for a loopback bind without a configured token.
    /// A configured token or a non-loopback bind keeps bearer authorization.
    pub fn local_mutations_without_token(&self) -> bool {
        self.local_mutations_without_token
    }

    pub fn allow_local_mutations_without_token(mut self) -> Self {
        self.local_mutations_without_token =
            self.auth_token.is_none() && self.bind_addr.ip().is_loopback();
        self
    }

    pub fn validate(self) -> Result<Self, ReviewServerConfigError> {
        if !self.bind_addr.ip().is_loopback() && self.auth_token.is_none() {
            return Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback);
        }
        Ok(self)
    }
}

const MAX_CONCURRENT_CONNECTIONS: usize = 32;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_IO_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
/// Live inspection shares the scraper's per-domain pacing, so a shop's
/// robots.txt `Crawl-delay` can queue it; it gives up in time to answer
/// within [`REQUEST_DEADLINE`].
const LIVE_FETCH_DEADLINE: Duration = Duration::from_secs(25);
/// A matrix refresh fetches every review page at the shop's `Crawl-delay`, so it runs in
/// the background with a budget per page: the longest honoured delay plus a full fetch.
const MATRIX_REFRESH_PAGE_BUDGET: Duration =
    MAX_CRAWL_DELAY.saturating_add(Duration::from_secs(30));
/// How long a refresh request waits for the matrix before answering that it is still
/// refreshing, leaving room within [`REQUEST_DEADLINE`].
const MATRIX_REFRESH_RESPONSE_WAIT: Duration = Duration::from_secs(20);
const RESPONSE_WRITE_DEADLINE: Duration = Duration::from_secs(10);
const REVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const REVIEW_SESSION_COOKIE: &str = "crawler_review_session";

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    domain_administration: Arc<dyn CrawlerDomainAdministration>,
    config: ReviewServerConfig,
    html_fetcher: Arc<dyn HtmlFetcher>,
    sessions: Arc<DashMap<uuid::Uuid, std::time::Instant>>,
    matrix_refreshes: MatrixRefreshes,
    matrix_refresh_wait: Duration,
}

impl ReviewServer {
    /// `html_fetcher` serves live inspection; pass the scraper's fetcher so
    /// both count against the same robots.txt cache and per-domain pacing.
    pub fn new(
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
            sessions: Arc::new(DashMap::new()),
            matrix_refreshes: MatrixRefreshes::default(),
            matrix_refresh_wait: MATRIX_REFRESH_RESPONSE_WAIT,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(
            bind_addr = %self.config.bind_addr,
            "Crawler review console listening"
        );

        let connection_limiter = connection_limiter();
        loop {
            let permit = connection_limiter
                .clone()
                .acquire_owned()
                .await
                .map_err(std::io::Error::other)?;
            let (stream, peer) = listener.accept().await?;
            let server = self.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(err) = server.handle_connection(stream).await {
                    warn!(peer = %peer, error = ?err, "Review console request failed");
                }
            });
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let Some(response) = self
            .response_for_request(&mut stream, REQUEST_DEADLINE)
            .await
        else {
            return Ok(());
        };
        let response = response.to_string();
        write_response(&mut stream, response.as_bytes(), RESPONSE_WRITE_DEADLINE).await
    }

    async fn response_for_request<R>(
        &self,
        stream: &mut R,
        deadline: Duration,
    ) -> Option<HttpResponse>
    where
        R: AsyncRead + Unpin,
    {
        match tokio::time::timeout(deadline, async {
            match read_bounded_request_headers(stream).await {
                Ok(Some(headers)) => {
                    let Some(request) = parse_request(&headers.head) else {
                        return Some(HttpResponse::text(400, "bad request"));
                    };
                    if !self.request_is_authorized(&request) {
                        return Some(HttpResponse::json(401, &json!({ "error": "unauthorized" })));
                    }
                    match read_bounded_request_body(stream, headers).await {
                        Ok(request) => Some(self.route(&request).await),
                        Err(error) => {
                            warn!(error = ?error, "Rejected malformed review console request");
                            Some(HttpResponse::json(400, &json!({ "error": "bad request" })))
                        }
                    }
                }
                Ok(None) => None,
                Err(error) => {
                    warn!(error = ?error, "Rejected malformed review console request");
                    Some(HttpResponse::json(400, &json!({ "error": "bad request" })))
                }
            }
        })
        .await
        {
            Ok(response) => response,
            Err(_) => {
                warn!("Review console request deadline exceeded");
                Some(HttpResponse::json(
                    400,
                    &json!({ "error": "request timed out" }),
                ))
            }
        }
    }

    async fn route(&self, request: &str) -> HttpResponse {
        let Some(parsed) = parse_request(request) else {
            return HttpResponse::text(400, "bad request");
        };

        if !self.request_is_authorized(&parsed) {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        }

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("POST", "/api/session") => self.create_session(),
            ("POST", "/api/session/logout") => self.clear_session(&parsed),
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
                                "domain_id": domain.domain_id,
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
                        "domain_id": domain.domain_id,
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
                        "domain_id": removal.domain_id,
                        "removed_url_count": removal.removed_url_count,
                        "removed_url_pattern_review_count": removal.removed_url_pattern_review_count,
                    }),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        // Preview responses, including their errors, are framed by the console.
        if request.method == "GET" && request.path == "/api/live-inspect" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }))
                    .into_review_preview();
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::html(200, &instrument_live_html(url, &html)),
                Err(response) => response,
            }
            .into_review_preview();
        }

        if request.method == "GET" && request.path == "/api/live-html" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }));
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::text(200, &html),
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
                match self.matrix_refreshes.status(review_id) {
                    MatrixRefreshStatus::Running => return matrix_refreshing(review_id),
                    MatrixRefreshStatus::Failed(response) => return response,
                    MatrixRefreshStatus::Idle => {}
                }
                match self.cached_schema_matrix(review_id).await {
                    Ok(Some(matrix)) => return HttpResponse::json(200, &matrix),
                    Ok(None) => {}
                    Err(err) => return internal_error(err),
                }
            }
            return self.refresh_schema_matrix(review_id).await;
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/pages")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/pages") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: ReviewPagePayload = match serde_json::from_str(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            let url = match Url::parse(&payload.url) {
                Ok(url) => url,
                Err(error) => {
                    return HttpResponse::json(
                        400,
                        &json!({ "error": "invalid review page url", "details": error.to_string() }),
                    );
                }
            };
            return match self.add_reviewer_page(review_id, url).await {
                Ok(page) => HttpResponse::json(201, &page),
                Err(response) => response,
            };
        }

        if request.method == "DELETE" && request.path.starts_with("/api/review-pages/") {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "") else {
                return HttpResponse::json(400, &json!({ "error": "invalid page id" }));
            };
            return match self.repository.remove_reviewer_page(page_id).await {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(error) => reviewer_page_error(error),
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/inspect")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/inspect") else {
                return HttpResponse::text(400, "invalid page id").into_review_preview();
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((page, html))) => {
                    HttpResponse::html(200, &instrument_review_page(&page, &html))
                }
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(response) => response,
            }
            .into_review_preview();
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/html")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/html") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((_page, html))) => HttpResponse::text(200, &html),
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
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .approve_review(review_id, payload.notes.as_deref())
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::InvalidUrlPatternCandidate)
                | Err(err @ ReviewRepositoryError::InvalidProductSchemaCandidate)
                | Err(err @ ReviewRepositoryError::InvalidRemovedPageTemplate { .. })
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
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
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .reject_review(review_id, payload.notes.as_deref())
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
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .request_repair(review_id, payload.notes.as_deref())
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::MissingRepairNote)
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
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
                | Err(err @ ReviewRepositoryError::InvalidProductSchemaCandidate)
                | Err(err @ ReviewRepositoryError::InvalidRemovedPageTemplate { .. })
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err @ ReviewRepositoryError::StaleApprovedReview(_))
                | Err(err @ ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(_)) => {
                    HttpResponse::json(409, &json!({ "error": err.to_string() }))
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
                | Err(err @ ReviewRepositoryError::InvalidProductSchemaCandidate)
                | Err(err @ ReviewRepositoryError::InvalidRemovedPageTemplate { .. })
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err @ ReviewRepositoryError::StaleApprovedReview(_))
                | Err(err @ ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(_)) => {
                    HttpResponse::json(409, &json!({ "error": err.to_string() }))
                }
                Err(err) => internal_error(err),
            };
        }

        HttpResponse::json(404, &json!({ "error": "not found" }))
    }

    fn request_is_authorized(&self, request: &ParsedRequest<'_>) -> bool {
        if !request.path.starts_with("/api/") {
            return true;
        }

        let bearer = self.authorized(&request.headers);
        let mutation_bearer = self.config.auth_token.is_some() && bearer;
        let session = self.has_valid_session(&request.headers);
        let local_mutation = self.config.local_mutations_without_token
            && self.config.auth_token.is_none()
            && is_same_origin_loopback_request(&request.headers);
        match (request.method.as_str(), request.path) {
            ("POST", "/api/session") => mutation_bearer,
            ("POST", "/api/session/logout") => mutation_bearer || session,
            (method, _) if is_mutation_method(method) => mutation_bearer || local_mutation,
            _ => bearer || session,
        }
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

    fn has_valid_session(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(session_id) = session_id_from_headers(headers) else {
            return false;
        };
        let now = std::time::Instant::now();
        match self.sessions.get(&session_id) {
            Some(expiry) if *expiry > now => true,
            Some(_) => {
                self.sessions.remove(&session_id);
                false
            }
            None => false,
        }
    }

    fn create_session(&self) -> HttpResponse {
        let now = std::time::Instant::now();
        self.sessions.retain(|_, expiry| *expiry > now);
        let session_id = uuid::Uuid::new_v4();
        self.sessions.insert(session_id, now + REVIEW_SESSION_TTL);
        HttpResponse::json(200, &json!({ "ok": true })).with_header(
            "Set-Cookie",
            session_cookie(&self.config, session_id, REVIEW_SESSION_TTL),
        )
    }

    fn clear_session(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        if let Some(session_id) = session_id_from_headers(&request.headers) {
            self.sessions.remove(&session_id);
        }
        HttpResponse::json(200, &json!({ "ok": true }))
            .with_header("Set-Cookie", expired_session_cookie(&self.config))
    }

    /// Fetches every review page live within a deadline sized for the shop's pacing.
    async fn live_review_pages(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(internal_error)?;
        let deadline = matrix_refresh_deadline(detail.pages.len());
        let fetch_all = async {
            let mut pages = Vec::with_capacity(detail.pages.len());
            for page in detail.pages {
                let url = review_page_url(&page)?;
                let html = self
                    .html_fetcher
                    .fetch(&url)
                    .await
                    .map(|fetched| fetched.html)
                    .map_err(|err| live_fetch_error(&page, &err))?;
                pages.push((page, html));
            }
            Ok(pages)
        };
        tokio::time::timeout(deadline, fetch_all)
            .await
            .unwrap_or_else(|_| Err(matrix_refresh_timeout(review_id, deadline)))
    }

    /// The review's cached matrix with the cross-page findings of the current checks, or
    /// `None` when no matrix is cached or the cached one does not load, so the caller
    /// refreshes it from the live pages.
    async fn cached_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Option<SchemaMatrix>, ReviewRepositoryError> {
        let detail = self.repository.get_review(review_id).await?;
        let Some(value) = detail.review.validation_summary.get("schema_matrix") else {
            return Ok(None);
        };
        match serde_json::from_value::<SchemaMatrix>(value.clone()) {
            Ok(mut matrix) => {
                evaluate_cross_page_findings(&mut matrix);
                Ok(Some(matrix))
            }
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

    /// Starts a background refresh, or joins the running one, and answers with the matrix
    /// when it finishes in time, otherwise with `202` for the console to poll.
    async fn refresh_schema_matrix(&self, review_id: CrawlerReviewId) -> HttpResponse {
        let refresh = self.clone().evaluate_live_schema_matrix(review_id);
        match self
            .matrix_refreshes
            .start_or_join(review_id, refresh)
            .outcome_within(self.matrix_refresh_wait)
            .await
        {
            Some(Ok(matrix)) => HttpResponse::json(200, &matrix),
            Some(Err(response)) => response,
            None => matrix_refreshing(review_id),
        }
    }

    async fn evaluate_live_schema_matrix(self, review_id: CrawlerReviewId) -> MatrixRefreshOutcome {
        let pages = self.live_review_pages(review_id).await?;
        let evaluated = self
            .repository
            .evaluate_schema_matrix_for_live_pages(review_id, pages)
            .await
            .map_err(internal_error)?;
        self.repository
            .store_schema_matrix_if_current(review_id, &evaluated)
            .await
            .map_err(schema_matrix_store_error)?;
        Ok(evaluated.matrix)
    }

    /// Fetches a reviewer-chosen page of the review's ListingSource and stores it as a review
    /// page. The target is checked before the fetch so the console never fetches a foreign
    /// host, and the final URL after redirects is checked again before it is stored.
    async fn add_reviewer_page(
        &self,
        review_id: CrawlerReviewId,
        url: Url,
    ) -> Result<CrawlerReviewPage, HttpResponse> {
        self.repository
            .check_reviewer_page_target(review_id, &url)
            .await
            .map_err(reviewer_page_error)?;
        let fetched = self
            .html_fetcher
            .fetch(&url)
            .await
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))?;
        self.repository
            .add_reviewer_page(review_id, &url, &fetched.final_url, &fetched.html)
            .await
            .map_err(reviewer_page_error)
    }

    async fn live_review_page(
        &self,
        review_page_id: CrawlerReviewPageId,
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
        let url = review_page_url(page)?;
        self.fetch_live(&url)
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
        self.fetch_live(&url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_url_fetch_error(raw_url, &err))
    }

    async fn fetch_live(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
        tokio::time::timeout(LIVE_FETCH_DEADLINE, self.html_fetcher.fetch(url))
            .await
            .unwrap_or_else(|_| {
                Err(FetchError::Network {
                    kind: NetworkErrorKind::Timeout,
                    details: format!(
                        "live fetch did not finish within {}s; the shop's robots.txt Crawl-delay may be queueing it behind the scraper",
                        LIVE_FETCH_DEADLINE.as_secs()
                    ),
                })
            })
    }
}

fn connection_limiter() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS))
}

async fn write_response<W>(
    writer: &mut W,
    response: &[u8],
    deadline: Duration,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(deadline, async {
        writer.write_all(response).await?;
        writer.shutdown().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "response write timed out"))?
}

struct BoundedRequestHeaders {
    head: String,
    buffered_body: Vec<u8>,
    content_length: usize,
}

async fn read_bounded_request_headers<R>(
    stream: &mut R,
) -> std::io::Result<Option<BoundedRequestHeaders>>
where
    R: AsyncRead + Unpin,
{
    let mut request = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
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
        let read = tokio::time::timeout(REQUEST_IO_TIMEOUT, stream.read(&mut byte))
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
        request.push(byte[0]);
    };
    let head = String::from_utf8(request[..header_end].to_vec()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request headers are not UTF-8",
        )
    })?;
    let mut content_length = None;
    for line in head.split("\r\n").skip(1) {
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
    Ok(Some(BoundedRequestHeaders {
        head,
        buffered_body: Vec::new(),
        content_length: content_length.unwrap_or(0),
    }))
}

async fn read_bounded_request_body<R>(
    stream: &mut R,
    mut headers: BoundedRequestHeaders,
) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut read_buffer = [0_u8; 4096];
    while headers.buffered_body.len() < headers.content_length {
        let remaining = headers.content_length - headers.buffered_body.len();
        let read_limit = read_buffer.len().min(remaining);
        let read = tokio::time::timeout(
            REQUEST_IO_TIMEOUT,
            stream.read(&mut read_buffer[..read_limit]),
        )
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
        headers
            .buffered_body
            .extend_from_slice(&read_buffer[..read]);
    }
    headers.buffered_body.truncate(headers.content_length);
    let body = String::from_utf8(headers.buffered_body).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "request is not UTF-8")
    })?;
    Ok(headers.head + &body)
}

#[cfg(test)]
async fn read_bounded_request<R>(stream: &mut R) -> std::io::Result<Option<String>>
where
    R: AsyncRead + Unpin,
{
    let Some(headers) = read_bounded_request_headers(stream).await? else {
        return Ok(None);
    };
    read_bounded_request_body(stream, headers).await.map(Some)
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

#[derive(Deserialize)]
struct ReviewPagePayload {
    url: String,
}

fn parse_action_payload(body: &str) -> Result<ActionPayload, serde_json::Error> {
    if body.trim().is_empty() {
        return Ok(ActionPayload::default());
    }
    serde_json::from_str(body)
}

fn session_id_from_headers(
    headers: &std::collections::HashMap<String, String>,
) -> Option<uuid::Uuid> {
    headers
        .get("cookie")?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{REVIEW_SESSION_COOKIE}=")))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
}

fn session_cookie(config: &ReviewServerConfig, session_id: uuid::Uuid, ttl: Duration) -> String {
    let secure = if config.bind_addr.ip().is_loopback() {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{REVIEW_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        ttl.as_secs()
    )
}

fn expired_session_cookie(config: &ReviewServerConfig) -> String {
    session_cookie(config, uuid::Uuid::nil(), Duration::ZERO)
}

fn parse_review_id(path: &str) -> Option<CrawlerReviewId> {
    let id = path.strip_prefix("/api/reviews/")?;
    id.parse().ok()
}

fn parse_review_id_with_suffix(path: &str, suffix: &str) -> Option<CrawlerReviewId> {
    let without_suffix = path.strip_suffix(suffix)?;
    parse_review_id(without_suffix)
}

fn parse_listing_source_id_with_suffix(path: &str, suffix: &str) -> Option<ListingSourceId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/listing-sources/")?;
    id.parse().ok()
}

fn parse_listing_source_domain_id(path: &str) -> Option<(ListingSourceId, CrawlerDomainId)> {
    let rest = path.strip_prefix("/api/listing-sources/")?;
    let (listing_source_id, domain_id) = rest.split_once("/domains/")?;
    Some((listing_source_id.parse().ok()?, domain_id.parse().ok()?))
}

fn parse_page_id_with_suffix(path: &str, suffix: &str) -> Option<CrawlerReviewPageId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/review-pages/")?;
    id.parse().ok()
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
        CrawlerDomainConfigurationError::UnsafeDomain { .. }
        | CrawlerDomainConfigurationError::RepeatedWwwPrefix { .. } => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::Database { .. } => internal_error(error),
    }
}

fn review_page_url(page: &CrawlerReviewPage) -> Result<Url, HttpResponse> {
    Url::parse(&page.url).map_err(|err| {
        HttpResponse::json(
            400,
            &json!({
                "error": "invalid live review page url",
                "url": page.url,
                "details": err.to_string(),
            }),
        )
    })
}

fn matrix_refresh_deadline(page_count: usize) -> Duration {
    MATRIX_REFRESH_PAGE_BUDGET.saturating_mul(u32::try_from(page_count.max(1)).unwrap_or(u32::MAX))
}

fn matrix_refreshing(review_id: CrawlerReviewId) -> HttpResponse {
    HttpResponse::json(
        202,
        &json!({
            "status": "refreshing",
            "review_id": review_id,
            "message": "live review pages are fetched at the shop's robots.txt Crawl-delay; poll the matrix without refresh",
        }),
    )
}

fn matrix_refresh_timeout(review_id: CrawlerReviewId, deadline: Duration) -> HttpResponse {
    warn!(review_id = %review_id, deadline_secs = deadline.as_secs(), "Schema matrix refresh timed out");
    HttpResponse::json(
        502,
        &json!({
            "error": "schema matrix refresh timed out",
            "details": format!("live review pages did not finish within {}s", deadline.as_secs()),
        }),
    )
}

fn schema_matrix_store_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::CandidateChangedDuringEvaluation => HttpResponse::json(
            409,
            &json!({ "error": "review candidate or pages changed; refresh and evaluate again" }),
        ),
        other => internal_error(other),
    }
}

fn reviewer_page_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::NotFound(_) | ReviewRepositoryError::ReviewPageNotFound(_) => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::DuplicateReviewPage(_) => {
            HttpResponse::json(409, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::ReviewPageUrlNotAllowed(_)
        | ReviewRepositoryError::ReviewPageLimitReached(_)
        | ReviewRepositoryError::NotReviewerAddedPage(_)
        | ReviewRepositoryError::RedirectedToNonProductPage(_)
        | ReviewRepositoryError::UnsupportedArtifact(_, _)
        | ReviewRepositoryError::NotPending(_) => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        other => internal_error(other),
    }
}

fn internal_error(error: impl std::fmt::Display + std::fmt::Debug) -> HttpResponse {
    error!(error = ?error, "Review API error");
    HttpResponse::json(500, &json!({ "error": "internal server error" }))
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// Browsers send `Origin` on every cross-origin mutation, so requiring it to equal
/// the loopback `Host` rejects other sites and DNS-rebinding hosts.
fn is_same_origin_loopback_request(headers: &std::collections::HashMap<String, String>) -> bool {
    let (Some(host), Some(origin)) = (headers.get("host"), headers.get("origin")) else {
        return false;
    };
    host_is_loopback(host) && origin.eq_ignore_ascii_case(&format!("http://{host}"))
}

fn host_is_loopback(host: &str) -> bool {
    let Ok(url) = Url::parse(&format!("http://{host}")) else {
        return false;
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
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
    use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
    use crate::service::crawler_domain_configuration::{
        CrawlerDomainConfiguration, CrawlerDomainRemoval,
    };

    struct RejectingDomainAdministration;

    struct StaticHtmlFetcher;

    #[async_trait::async_trait]
    impl HtmlFetcher for StaticHtmlFetcher {
        async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
            Ok(FetchedHtml {
                html: "<script>alert('remote')</script>".to_string(),
                final_url: url.clone(),
                status: reqwest::StatusCode::OK,
            })
        }
    }

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
                local_mutations_without_token: false,
            },
            Arc::new(MockHtmlFetcher::new()),
        ))
    }

    #[tokio::test]
    async fn should_limit_review_connections() -> Result<(), Box<dyn std::error::Error>> {
        let limiter = connection_limiter();
        let mut permits = Vec::with_capacity(MAX_CONCURRENT_CONNECTIONS);
        for _ in 0..MAX_CONCURRENT_CONNECTIONS {
            permits.push(limiter.clone().acquire_owned().await?);
        }

        assert!(limiter.clone().try_acquire_owned().is_err());
        drop(permits);
        assert!(limiter.clone().try_acquire_owned().is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_total_request_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(None).await?;
        let (_client, mut stream) = tokio::io::duplex(1024);

        let response = server
            .response_for_request(&mut stream, Duration::from_millis(10))
            .await;

        let response = match response {
            Some(response) => response,
            None => return Err("request deadline returned no response".into()),
        };
        assert!(response.to_string().starts_with("HTTP/1.1 400 Bad Request"));
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_response_write_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let (mut writer, _reader) = tokio::io::duplex(1);

        let error = match write_response(&mut writer, b"response", Duration::from_millis(10)).await
        {
            Ok(()) => return Err("response write should time out".into()),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "response write timed out");
        Ok(())
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
    async fn should_authenticate_api_headers_before_waiting_for_declared_body()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(Some("review-token")).await?;
        let (mut client, mut stream) = tokio::io::duplex(1024);
        client
            .write_all(b"POST /api/health HTTP/1.1\r\nContent-Length: 64\r\n\r\n")
            .await?;

        let response = tokio::time::timeout(
            Duration::from_millis(100),
            server.response_for_request(&mut stream, Duration::from_secs(1)),
        )
        .await?
        .ok_or("unauthorized request returned no response")?;

        assert!(
            response
                .to_string()
                .starts_with("HTTP/1.1 401 Unauthorized")
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_header_overshoot() -> Result<(), Box<dyn std::error::Error>> {
        let prefix = "GET / HTTP/1.1\r\nX-Long: ";
        let request = format!(
            "{prefix}{}\r\n\r\n",
            "a".repeat(MAX_REQUEST_HEADER_BYTES - prefix.len() + 1)
        );

        let error = read_request_from_socket_chunks(&[request.as_bytes()])
            .await
            .expect_err("oversized request headers should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "request headers exceed limit");
        Ok(())
    }

    #[tokio::test]
    async fn should_serve_raw_live_html_as_plain_text() -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let server = ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                local_mutations_without_token: false,
            },
            Arc::new(StaticHtmlFetcher),
        );

        let response = server
            .route("GET /api/live-html?url=https%3A%2F%2Fexample.com HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(response.ends_with("<script>alert('remote')</script>"));
        assert!(response.contains("frame-ancestors 'none'"));
        Ok(())
    }

    #[tokio::test]
    async fn should_let_only_the_review_console_frame_live_inspection()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let server = ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                local_mutations_without_token: false,
            },
            Arc::new(StaticHtmlFetcher),
        );

        let inspection = server
            .route("GET /api/live-inspect?url=https%3A%2F%2Fexample.com HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let console = server.route("GET / HTTP/1.1\r\n\r\n").await.to_string();

        let missing_url = server
            .route("GET /api/live-inspect HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let invalid_page = server
            .route("GET /api/review-pages/not-a-page/inspect HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(inspection.starts_with("HTTP/1.1 200 OK"));
        for preview in [&inspection, &missing_url, &invalid_page] {
            assert!(preview.contains("Content-Security-Policy: sandbox allow-scripts;"));
            assert!(preview.contains("frame-ancestors 'self'\r\n"));
        }
        assert!(!console.contains("sandbox"));
        assert!(console.contains("frame-ancestors 'none'\r\n"));
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

    #[tokio::test]
    async fn should_allow_cookie_session_for_protected_navigation_and_invalidate_it_on_logout()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let server = ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: Some("review-token".to_string()),
                local_mutations_without_token: false,
            },
            Arc::new(StaticHtmlFetcher),
        );
        let login = server
            .route("POST /api/session HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        let cookie = login
            .lines()
            .find_map(|line| line.strip_prefix("Set-Cookie: "))
            .ok_or("session response did not set a cookie")?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?;

        assert!(login.contains("HttpOnly"));
        assert!(login.contains("SameSite=Strict"));
        assert!(!login.contains("review-token"));

        let reviews_raw_request = format!("GET /api/reviews HTTP/1.1\r\nCookie: {cookie}\r\n\r\n");
        let reviews_request =
            parse_request(&reviews_raw_request).ok_or("reviews request should parse")?;
        assert!(server.request_is_authorized(&reviews_request));

        let navigation = server
            .route(&format!(
                "GET /api/live-inspect?url=https%3A%2F%2Fexample.com HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(navigation.starts_with("HTTP/1.1 200 OK"));
        assert!(navigation.contains("data-crawler-review-disabled"));

        for request in [
            format!("POST /api/session HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
            format!(
                "POST /api/reviews/{}/approve HTTP/1.1\r\nCookie: {cookie}\r\n\r\n",
                uuid::Uuid::new_v4()
            ),
            format!(
                "POST /api/listing-sources/{}/domains HTTP/1.1\r\nCookie: {cookie}\r\n\r\n",
                uuid::Uuid::new_v4()
            ),
        ] {
            let response = server.route(&request).await.to_string();
            assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        }

        let bearer_mutation = server
            .route("POST /api/session HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        assert!(bearer_mutation.starts_with("HTTP/1.1 200 OK"));

        let logout = server
            .route(&format!(
                "POST /api/session/logout HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(logout.starts_with("HTTP/1.1 200 OK"));
        assert!(logout.contains("Max-Age=0"));

        let expired = server
            .route(&format!(
                "GET /api/health HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(expired.starts_with("HTTP/1.1 401 Unauthorized"));
        Ok(())
    }

    async fn local_demo_review_server_for_test() -> Result<ReviewServer, sqlx::Error> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        Ok(ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                local_mutations_without_token: false,
            }
            .allow_local_mutations_without_token(),
            Arc::new(MockHtmlFetcher::new()),
        ))
    }

    #[tokio::test]
    async fn should_allow_same_origin_loopback_mutations_without_token_in_local_mode()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = local_demo_review_server_for_test().await?;

        for (host, origin) in [
            ("127.0.0.1:7878", "http://127.0.0.1:7878"),
            ("localhost:7878", "http://localhost:7878"),
            ("[::1]:7878", "http://[::1]:7878"),
        ] {
            let response = server
                .route(&format!(
                    "POST /api/health HTTP/1.1
Host: {host}
Origin: {origin}

"
                ))
                .await
                .to_string();
            assert!(response.starts_with("HTTP/1.1 404"), "{host}: {response}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cross_origin_or_rebound_mutations_in_local_mode()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = local_demo_review_server_for_test().await?;

        for headers in [
            "Host: 127.0.0.1:7878
",
            "Origin: http://127.0.0.1:7878
",
            "Host: 127.0.0.1:7878
Origin: https://evil.example
",
            "Host: 127.0.0.1:7878
Origin: null
",
            "Host: evil.example:7878
Origin: http://evil.example:7878
",
            "Host: 127.0.0.1:7878/x
Origin: http://127.0.0.1:7878/x
",
        ] {
            let response = server
                .route(&format!(
                    "POST /api/health HTTP/1.1
{headers}
"
                ))
                .await
                .to_string();
            assert!(
                response.starts_with("HTTP/1.1 401"),
                "{headers}: {response}"
            );
        }
        Ok(())
    }

    #[test]
    fn should_keep_bearer_authorization_when_local_mode_has_token_or_public_bind() {
        let with_token = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();
        let public_bind = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();

        assert!(!with_token.local_mutations_without_token);
        assert!(!public_bind.local_mutations_without_token);
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
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
            local_mutations_without_token: false,
        }
        .validate();

        assert!(result.is_ok());
    }

    #[test]
    fn should_allow_loopback_review_bind_without_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
        }
        .validate();

        assert!(result.is_ok());
    }

    // Tests that touch the process environment are serialized, so no test reads the
    // environment while another one changes it.
    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    const REVIEW_BIND_ADDR_ENV: &str = "CRAWLER_REVIEW_BIND_ADDR";
    const REVIEW_AUTH_TOKEN_ENV: &str = "CRAWLER_REVIEW_AUTH_TOKEN";

    /// Reads the review config from exactly these review settings, leaving them unset afterwards.
    fn review_config_from(
        bind_addr: Option<&str>,
        auth_token: Option<&str>,
    ) -> Result<ReviewServerConfig, ReviewServerConfigError> {
        set_env(REVIEW_BIND_ADDR_ENV, bind_addr);
        set_env(REVIEW_AUTH_TOKEN_ENV, auth_token);
        let config = ReviewServerConfig::from_env();
        set_env(REVIEW_BIND_ADDR_ENV, None);
        set_env(REVIEW_AUTH_TOKEN_ENV, None);
        config
    }

    #[rstest::rstest]
    #[case::unset(None)]
    #[case::blank(Some("  "))]
    #[serial_test::serial]
    fn should_name_the_missing_token_when_a_non_loopback_bind_has_none(
        #[case] auth_token: Option<&'static str>,
    ) {
        let error = review_config_from(Some("0.0.0.0:7878"), auth_token)
            .err()
            .unwrap_or_else(|| panic!("a non-loopback bind without a token must fail"));

        assert_eq!(
            error.to_string(),
            "CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses"
        );
    }

    #[test]
    #[serial_test::serial]
    fn should_name_the_bind_address_when_it_is_not_host_and_port() {
        let error = review_config_from(Some("localhost"), None)
            .err()
            .unwrap_or_else(|| panic!("a bind address without a port must fail"));

        assert_eq!(
            error.to_string(),
            "CRAWLER_REVIEW_BIND_ADDR must be host:port"
        );
    }

    #[tokio::test]
    async fn should_expose_minimal_unauthenticated_liveness_health()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(Some("review-token")).await?;

        let response = server
            .route("GET /health HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("{\n  \"ok\": true\n}"));
        Ok(())
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

    mod matrix_refresh {
        use super::{ReviewServer, ReviewServerConfig};
        use crate::network::policy::NetworkErrorKind;
        use crate::review::matrix_refresh::MatrixRefreshStatus;
        use crate::review::model::{PAGE_ROLE_PRIMARY, SchemaReviewPageInput};
        use crate::review::repository::CrawlerReviewRepository;
        use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
        use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
        use crate::scraper::css_selector::rule::{
            ExtractionCardinality, ExtractionKind, ExtractionRule,
        };
        use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
        use crate::service::crawler_domain_configuration::{
            CrawlerDomainAdministration, CrawlerDomainConfiguration,
            CrawlerDomainConfigurationError, CrawlerDomainRemoval,
        };
        use crate::{CrawlerDomainId, CrawlerReviewId};
        use listing_source_core::{Domain, ListingSourceId};
        use serde_json::json;
        use sqlx::PgPool;
        use std::collections::HashMap;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        use test_api::IntegrationTestService as _;
        use tokio::sync::Semaphore;
        use url::Url;

        const PAGE_HTML: &str = "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>";

        /// A shop whose pages are served only as the test releases them, like requests queued
        /// behind a robots.txt `Crawl-delay`.
        struct PacedFetcher {
            released: Semaphore,
            fetched: AtomicUsize,
            failure: Option<&'static str>,
        }

        impl PacedFetcher {
            fn new(failure: Option<&'static str>) -> Arc<Self> {
                Arc::new(Self {
                    released: Semaphore::new(0),
                    fetched: AtomicUsize::new(0),
                    failure,
                })
            }

            fn release(&self, pages: usize) {
                self.released.add_permits(pages);
            }
        }

        #[async_trait::async_trait]
        impl HtmlFetcher for PacedFetcher {
            async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
                let permit =
                    self.released
                        .acquire()
                        .await
                        .map_err(|error| FetchError::Network {
                            kind: NetworkErrorKind::Connect,
                            details: error.to_string(),
                        })?;
                permit.forget();
                self.fetched.fetch_add(1, Ordering::SeqCst);
                if let Some(failure) = self.failure {
                    return Err(FetchError::Network {
                        kind: NetworkErrorKind::RobotsDisallowed,
                        details: failure.to_string(),
                    });
                }
                Ok(FetchedHtml {
                    html: PAGE_HTML.to_string(),
                    final_url: url.clone(),
                    status: reqwest::StatusCode::OK,
                })
            }
        }

        struct NoDomainAdministration;

        #[async_trait::async_trait]
        impl CrawlerDomainAdministration for NoDomainAdministration {
            async fn list_crawler_domains(
                &self,
                listing_source_id: ListingSourceId,
            ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError>
            {
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

        fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
            ExtractionRule {
                selector: selector.into(),
                additional_selectors: vec![],
                extract,
                cardinality: ExtractionCardinality::First,
            }
        }

        pub(super) fn schema(title_selector: &str) -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                source_listing_id: Some(rule("span.id", ExtractionKind::Text)),
                title: rule(title_selector, ExtractionKind::Text),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: rule("span.state", ExtractionKind::Text),
                images: rule(
                    "img.product",
                    ExtractionKind::Attribute { name: "src".into() },
                ),
                raw_attributes: Default::default(),
            }
        }

        struct Fixture {
            pool: PgPool,
            server: ReviewServer,
            review_id: CrawlerReviewId,
        }

        const PAGE_COUNT: usize = 2;

        async fn fixture(fetcher: Arc<PacedFetcher>) -> Fixture {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Paced source', 'paced-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("insert listing source: {error}"));
            let repository = CrawlerReviewRepository::new(pool.clone());
            let pages = (1..=PAGE_COUNT)
                .map(|index| SchemaReviewPageInput {
                    url: format!("https://paced.example/products/{index}"),
                    role: PAGE_ROLE_PRIMARY.to_string(),
                    raw_html: PAGE_HTML.to_string(),
                })
                .collect();
            let review_id = repository
                .create_schema_review(
                    &listing_source_id,
                    "manual_schema_required",
                    &[schema("h1")],
                    pages,
                    json!({}),
                )
                .await
                .unwrap_or_else(|error| panic!("create schema review: {error}"));
            let server = ReviewServer {
                matrix_refresh_wait: Duration::from_millis(50),
                ..ReviewServer::new(
                    repository,
                    Arc::new(NoDomainAdministration),
                    ReviewServerConfig {
                        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                        auth_token: None,
                        local_mutations_without_token: false,
                    },
                    fetcher.clone(),
                )
            };
            register_refresh_shutdown(server.clone(), fetcher, review_id);
            Fixture {
                pool,
                server,
                review_id,
            }
        }

        fn register_refresh_shutdown(
            server: ReviewServer,
            fetcher: Arc<PacedFetcher>,
            review_id: CrawlerReviewId,
        ) {
            test_api::TestContext::current()
                .expect("matrix refresh test context")
                .on_shutdown(async move {
                    // A failed assertion can leave the refresh waiting for the fake shop.
                    // Finish its database writes before the context removes its database.
                    fetcher.release(PAGE_COUNT);
                    while matches!(
                        server.matrix_refreshes.status(review_id),
                        MatrixRefreshStatus::Running
                    ) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Ok(())
                })
                .expect("register matrix refresh shutdown");
        }

        async fn get_matrix(
            server: &ReviewServer,
            review_id: CrawlerReviewId,
            refresh: bool,
        ) -> String {
            let query = if refresh { "?refresh=true" } else { "" };
            server
                .route(&format!(
                    "GET /api/reviews/{review_id}/matrix{query} HTTP/1.1\r\nHost: localhost\r\n\r\n"
                ))
                .await
                .to_string()
        }

        /// Polls the matrix without `refresh` until the background refresh has an answer.
        async fn poll_until_settled(server: &ReviewServer, review_id: CrawlerReviewId) -> String {
            for _ in 0..200 {
                let response = get_matrix(server, review_id, false).await;
                if !response.starts_with("HTTP/1.1 202") {
                    return response;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("matrix refresh did not settle");
        }

        async fn stored_matrix(
            pool: &PgPool,
            review_id: CrawlerReviewId,
        ) -> Option<serde_json::Value> {
            let summary: serde_json::Value = sqlx::query_scalar(
                "SELECT validation_summary FROM crawler_reviews WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap_or_else(|error| panic!("read validation summary: {error}"));
            summary.get("schema_matrix").cloned()
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_accepted_and_serve_the_matrix_on_poll_when_pages_are_paced() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;

            let started = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(started.starts_with("HTTP/1.1 202 Accepted"), "{started}");
            assert!(started.contains("\"status\": \"refreshing\""), "{started}");
            let polled = get_matrix(&fixture.server, fixture.review_id, false).await;
            assert!(polled.starts_with("HTTP/1.1 202"), "{polled}");

            fetcher.release(PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, fixture.review_id).await;

            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            assert!(
                settled.contains(&fixture.review_id.to_string()),
                "{settled}"
            );
            assert!(
                stored_matrix(&fixture.pool, fixture.review_id)
                    .await
                    .is_some()
            );
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_fetch_each_page_once_when_refresh_requests_overlap() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;

            let first = get_matrix(&fixture.server, fixture.review_id, true).await;
            let second = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(first.starts_with("HTTP/1.1 202"), "{first}");
            assert!(second.starts_with("HTTP/1.1 202"), "{second}");

            fetcher.release(2 * PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, fixture.review_id).await;

            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            assert_eq!(fetcher.fetched.load(Ordering::SeqCst), PAGE_COUNT);
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_report_the_page_fetch_failure_on_poll_when_the_refresh_fails() {
            let fetcher = PacedFetcher::new(Some("robots.txt disallows the page"));
            let fixture = fixture(fetcher.clone()).await;

            let started = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(started.starts_with("HTTP/1.1 202"), "{started}");
            fetcher.release(PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, fixture.review_id).await;

            assert!(settled.starts_with("HTTP/1.1 502"), "{settled}");
            assert!(
                settled.contains("robots.txt disallows the page"),
                "{settled}"
            );
            assert!(
                stored_matrix(&fixture.pool, fixture.review_id)
                    .await
                    .is_none()
            );
        }

        /// A matrix cached before `NO_IMAGE_ON_ANY_PAGE` existed, of pages whose `images`
        /// rule yields only thumbnails capture rejects, shows the finding without fetching
        /// the pages again.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_show_a_cross_page_check_added_after_the_matrix_was_cached() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;
            let thumbnail_page = PAGE_HTML.replace("src=\"a.jpg\"", "src=\"a-100x100.jpg\"");
            let pages: Vec<_> = CrawlerReviewRepository::new(fixture.pool.clone())
                .get_review(fixture.review_id)
                .await
                .unwrap_or_else(|error| panic!("read review: {error}"))
                .pages
                .into_iter()
                .map(|page| (page, thumbnail_page.clone()))
                .collect();
            let evaluated = evaluate_schema_matrix_for_live_review_pages(
                fixture.review_id,
                &[schema("h1")],
                &[],
                &pages,
                &HashMap::new(),
            );
            let mut cached = serde_json::to_value(&evaluated)
                .unwrap_or_else(|error| panic!("a matrix must serialize: {error}"));
            let mut stripped = 0;
            for page in cached["candidates"][0]["pages"]
                .as_array_mut()
                .unwrap_or_else(|| panic!("the matrix has no pages"))
            {
                let findings = page["cross_page_findings"]
                    .as_array_mut()
                    .unwrap_or_else(|| panic!("a page has no cross-page findings"));
                let evaluated_findings = findings.len();
                findings.retain(|finding| finding["check"] != "NO_IMAGE_ON_ANY_PAGE");
                stripped += evaluated_findings - findings.len();
            }
            assert_eq!(
                stripped, PAGE_COUNT,
                "evaluated afresh, every page has the finding"
            );
            sqlx::query(
                "UPDATE crawler_reviews \
                 SET validation_summary = jsonb_set(validation_summary, '{schema_matrix}', $2::jsonb, true) \
                 WHERE review_id = $1",
            )
            .bind(fixture.review_id.as_uuid())
            .bind(cached)
            .execute(&fixture.pool)
            .await
            .unwrap_or_else(|error| panic!("cache the matrix: {error}"));

            let response = get_matrix(&fixture.server, fixture.review_id, false).await;

            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            assert!(response.contains("NO_IMAGE_ON_ANY_PAGE"), "{response}");
            assert_eq!(fetcher.fetched.load(Ordering::SeqCst), 0);
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_store_the_matrix_of_the_current_candidate_when_it_changes_during_the_refresh()
         {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;

            let started = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(started.starts_with("HTTP/1.1 202"), "{started}");
            CrawlerReviewRepository::new(fixture.pool.clone())
                .update_candidate_payload(fixture.review_id, json!({ "schemas": [schema("h2")] }))
                .await
                .unwrap_or_else(|error| panic!("edit candidate: {error}"));
            fetcher.release(PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, fixture.review_id).await;

            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            let stored = stored_matrix(&fixture.pool, fixture.review_id)
                .await
                .unwrap_or_else(|| panic!("matrix of the edited candidate must be stored"));
            let pages = stored["candidates"][0]["pages"]
                .as_array()
                .unwrap_or_else(|| panic!("stored matrix has no pages: {stored}"));
            assert_eq!(pages.len(), PAGE_COUNT);
            assert!(
                pages.iter().all(|page| page["apply_ok"] == json!(false)),
                "the edited `h2` title selector matches none of the fetched pages: {stored}"
            );
        }
    }

    mod needs_repair {
        use super::matrix_refresh::schema;
        use super::{RejectingDomainAdministration, ReviewServer, ReviewServerConfig};
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        const TOKEN: &str = "review-token";

        /// Sends an authorized request and returns its status line and JSON body.
        async fn send(
            server: &ReviewServer,
            request: &str,
            body: &str,
        ) -> (String, serde_json::Value) {
            let response = server
                .route(&format!(
                    "{request} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\r\n{body}"
                ))
                .await
                .to_string();
            let (head, json) = response
                .split_once("\r\n\r\n")
                .unwrap_or_else(|| panic!("response without body: {response}"));
            let status = head.lines().next().unwrap_or_default().to_owned();
            let json = serde_json::from_str(json)
                .unwrap_or_else(|error| panic!("response body is not JSON ({error}): {response}"));
            (status, json)
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_show_the_repair_note_on_the_review_api_and_refuse_a_blank_note() {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
                 VALUES ($1, 'Repair source', 'repair-source', TRUE, NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
            let repository = CrawlerReviewRepository::new(pool);
            let review_id = repository
                .create_schema_review(
                    &listing_source_id,
                    "manual_schema_required",
                    &[schema("h1")],
                    vec![],
                    json!({}),
                )
                .await
                .unwrap_or_else(|error| panic!("create schema review: {error}"));
            let server = ReviewServer::new(
                repository,
                Arc::new(RejectingDomainAdministration),
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    local_mutations_without_token: false,
                },
                Arc::new(MockHtmlFetcher::new()),
            );
            let needs_repair = format!("POST /api/reviews/{review_id}/needs-repair");

            for blank in ["", r#"{"notes": null}"#, r#"{"notes": " \n "}"#] {
                let (status, body) = send(&server, &needs_repair, blank).await;
                assert!(
                    status.starts_with("HTTP/1.1 400"),
                    "{blank}: {status} {body}"
                );
            }
            let (status, body) = send(
                &server,
                &needs_repair,
                r#"{"notes": " title misses sold pages "}"#,
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {body}");

            let (status, detail) =
                send(&server, &format!("GET /api/reviews/{review_id}"), "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {detail}");
            let review = &detail["review"];
            assert_eq!(review["status"], "PENDING_REVIEW");
            assert_eq!(review["repair_request"]["note"], "title misses sold pages");
            assert!(
                review["repair_request"]["requested"].is_string(),
                "{review}"
            );
            let (_, queue) = send(&server, "GET /api/reviews", "").await;
            assert_eq!(queue[0]["repair_request"], review["repair_request"]);

            let (status, body) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/reject"),
                "",
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {body}");
            let (status, body) = send(&server, &needs_repair, r#"{"notes": "too late"}"#).await;
            assert!(status.starts_with("HTTP/1.1 400"), "{status} {body}");
        }
    }
}
