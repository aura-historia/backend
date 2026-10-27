use crate::network::policy::NetworkErrorKind;
use crate::network::robots::MAX_CRAWL_DELAY;
use crate::review::access::ReviewAccess;
pub use crate::review::access::{ReviewServerConfig, ReviewServerConfigError};
use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::matrix_refresh::{MatrixRefreshOutcome, MatrixRefreshStatus, MatrixRefreshes};
use crate::review::model::{CrawlerReviewPage, RawAttributeKey, ReviewDetail, SchemaMatrix};
use crate::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crate::review::repository::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
use crate::service::crawler_domain_configuration::{
    CrawlerDomainAdministration, CrawlerDomainConfigurationError,
};
use crate::{CrawlerDomainId, CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::{Domain, ListingSourceId};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use url::Url;

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

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    domain_administration: Arc<dyn CrawlerDomainAdministration>,
    access: ReviewAccess,
    html_fetcher: Arc<dyn HtmlFetcher>,
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
            access: ReviewAccess::new(config),
            html_fetcher,
            matrix_refreshes: MatrixRefreshes::default(),
            matrix_refresh_wait: MATRIX_REFRESH_RESPONSE_WAIT,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let bind_addr = self.access.bind_addr();
        let listener = TcpListener::bind(bind_addr).await?;
        info!(
            bind_addr = %bind_addr,
            agent_access = self.access.agent_access(),
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
                    if self.access.authorize(&request).is_none() {
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

        if self.access.authorize(&parsed).is_none() {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        }

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("POST", "/api/session") => self.access.create_session(),
            ("POST", "/api/session/logout") => self.access.clear_session(&parsed),
            ("GET", "/api/listing_sources") => {
                match self.repository.list_listing_sources(200).await {
                    Ok(listing_sources) => HttpResponse::json(200, &listing_sources),
                    Err(err) => internal_error(err),
                }
            }
            ("GET", "/api/raw-attribute-keys") => {
                HttpResponse::json(200, &RawAttributeKey::registry())
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
            }
            // An unknown review is refused before a refresh starts, so requests for unknown ids
            // leave no refresh behind in the registry.
            let detail = match self.repository.get_review(review_id).await {
                Ok(detail) => detail,
                Err(err) => return review_error(err),
            };
            if !refresh && let Some(matrix) = cached_schema_matrix(&detail) {
                return HttpResponse::json(200, &matrix);
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
                Err(error) => review_error(error),
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/inspect")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/inspect") else {
                return HttpResponse::json(400, &json!({ "error": "invalid page id" }))
                    .into_review_preview();
            };
            return match self.live_review_page(page_id).await {
                Ok((page, html)) => HttpResponse::html(200, &instrument_review_page(&page, &html)),
                Err(response) => response,
            }
            .into_review_preview();
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/html")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/html") else {
                return HttpResponse::json(400, &json!({ "error": "invalid page id" }));
            };
            return match self.live_review_page(page_id).await {
                Ok((_page, html)) => HttpResponse::text(200, &html),
                Err(response) => response,
            };
        }

        if request.method == "GET" && request.path.starts_with("/api/reviews/") {
            let Some(review_id) = parse_review_id(request.path) else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            return match self.repository.get_review(review_id).await {
                Ok(detail) => HttpResponse::json(200, &detail),
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/approve")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/approve") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: ApprovalPayload = match parse_verdict_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .approve_review(
                    review_id,
                    payload.notes.as_deref(),
                    payload.incomplete_coverage(),
                )
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/reject")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/reject") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: ActionPayload = match parse_verdict_payload(request.body) {
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
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/needs-repair")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/needs-repair") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: ActionPayload = match parse_verdict_payload(request.body) {
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
                Err(err) => review_error(err),
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
                    payload.expected_candidate_version,
                    payload.schema_index,
                    &payload.field,
                    payload.rule,
                )
                .await
            {
                Ok(candidate_version) => candidate_edited(candidate_version),
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/candidate")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/candidate") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: CandidatePayload = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .update_candidate_payload(
                    review_id,
                    payload.expected_candidate_version,
                    serde_json::Value::Object(payload.candidate),
                )
                .await
            {
                Ok(candidate_version) => candidate_edited(candidate_version),
                Err(err) => review_error(err),
            };
        }

        HttpResponse::json(404, &json!({ "error": "not found" }))
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
            .map_err(review_error)?;
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
            .map_err(review_error)?;
        self.repository
            .store_schema_matrix_if_current(review_id, &evaluated)
            .await
            .map_err(review_error)?;
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
            .map_err(review_error)?;
        let fetched = self
            .html_fetcher
            .fetch(&url)
            .await
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))?;
        self.repository
            .add_reviewer_page(review_id, &url, &fetched.final_url, &fetched.html)
            .await
            .map_err(review_error)
    }

    async fn live_review_page(
        &self,
        review_page_id: CrawlerReviewPageId,
    ) -> Result<(CrawlerReviewPage, String), HttpResponse> {
        let page = self
            .repository
            .get_review_page(review_page_id)
            .await
            .map_err(internal_error)?
            .ok_or_else(|| {
                review_error(ReviewRepositoryError::ReviewPageNotFound(review_page_id))
            })?;
        let html = self.fetch_live_html(&page).await?;
        Ok((page, html))
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

/// An approval of `POST /api/reviews/{id}/approve`. `acknowledged_coverage` approves a
/// product-schema candidate that does not cover every review page while the review's coverage is
/// still the one it names; the console sends the coverage its confirmation dialog stated.
#[derive(Deserialize, Default)]
struct ApprovalPayload {
    notes: Option<String>,
    acknowledged_coverage: Option<ReviewPageCoverage>,
}

impl ApprovalPayload {
    fn incomplete_coverage(&self) -> IncompleteCoverage {
        self.acknowledged_coverage
            .map_or(IncompleteCoverage::Refuse, IncompleteCoverage::Acknowledged)
    }
}

/// A rule edit of `POST /api/reviews/{id}/schema-field`. The `rule` key is required: an explicit
/// `null` deletes the rule, while a left-out or misspelled key is refused instead of deleting it.
/// `expected_candidate_version` is the candidate version the edit was based on.
#[derive(Deserialize)]
struct SchemaFieldPayload {
    expected_candidate_version: i64,
    schema_index: usize,
    field: String,
    #[serde(deserialize_with = "Option::deserialize")]
    rule: Option<ExtractionRule>,
}

/// A whole-candidate edit of `POST /api/reviews/{id}/candidate`: the candidate's keys next to the
/// candidate version the edit was based on.
#[derive(Deserialize)]
struct CandidatePayload {
    expected_candidate_version: i64,
    #[serde(flatten)]
    candidate: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct DomainPayload {
    domain: String,
}

#[derive(Deserialize)]
struct ReviewPagePayload {
    url: String,
}

/// Reads the body of a verdict, where an empty body stands for a verdict without notes.
fn parse_verdict_payload<T: DeserializeOwned + Default>(
    body: &str,
) -> Result<T, serde_json::Error> {
    if body.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(body)
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

/// The schema matrix cached on the review, or `None` when it holds none, or one that no longer
/// decodes, so the caller refreshes it from the live pages.
fn cached_schema_matrix(detail: &ReviewDetail) -> Option<SchemaMatrix> {
    SchemaMatrix::stored_in(&detail.review.validation_summary)?
        .inspect_err(|err| {
            warn!(
                review_id = %detail.review.review_id,
                error = ?err,
                "Cached schema matrix is invalid; refreshing from live pages"
            );
        })
        .ok()
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

/// Answers a request the review changed under with 409 and the review's current candidate
/// version, the one shape of a conflict the caller resolves by reloading the review and retrying.
fn review_changed(error: &ReviewRepositoryError, candidate_version: i64) -> HttpResponse {
    HttpResponse::json(
        409,
        &json!({ "error": error.to_string(), "candidate_version": candidate_version }),
    )
}

/// Answers an accepted candidate edit (`/candidate` or `/schema-field`) with the review's
/// candidate version after it.
fn candidate_edited(candidate_version: i64) -> HttpResponse {
    HttpResponse::json(
        200,
        &json!({ "ok": true, "candidate_version": candidate_version }),
    )
}

/// Maps a refused review request to its answer, the same on every review route: 404 for an
/// unknown review or review page, 400 for a request the review cannot take, 409 for a request
/// the review's current state refuses, and 500 for a technical failure. A 409 for a request based
/// on an older candidate carries the review's current `candidate_version`, and one for an approval
/// of incomplete coverage, unacknowledged or acknowledged at other counts, carries the review's
/// current `coverage`.
fn review_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::NotFound(_) | ReviewRepositoryError::ReviewPageNotFound(_) => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::NotPending(_)
        | ReviewRepositoryError::MissingRepairNote
        | ReviewRepositoryError::UnsupportedArtifact(_, _)
        | ReviewRepositoryError::InvalidSchemaField(_)
        | ReviewRepositoryError::RequiredSchemaField(_)
        | ReviewRepositoryError::UnregisteredRawAttributeKey(_)
        | ReviewRepositoryError::InvalidUrlPatternCandidate
        | ReviewRepositoryError::InvalidProductSchemaCandidate
        | ReviewRepositoryError::InvalidRemovedPageTemplate { .. }
        | ReviewRepositoryError::ReviewPageUrlNotAllowed(_)
        | ReviewRepositoryError::ReviewPageLimitReached(_)
        | ReviewRepositoryError::NotReviewerAddedPage(_)
        | ReviewRepositoryError::RedirectedToNonProductPage(_) => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::StaleCandidateVersion { current, .. } => {
            review_changed(&error, current)
        }
        ReviewRepositoryError::CandidateChangedDuringEvaluation { candidate_version } => {
            review_changed(&error, candidate_version)
        }
        ReviewRepositoryError::StaleApprovedReview(_)
        | ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(_)
        | ReviewRepositoryError::DuplicateReviewPage(_) => {
            HttpResponse::json(409, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::IncompleteCoverage { coverage, .. }
        | ReviewRepositoryError::StaleCoverageAcknowledgement { coverage, .. } => {
            HttpResponse::json(
                409,
                &json!({ "error": error.to_string(), "coverage": coverage }),
            )
        }
        ReviewRepositoryError::Database(_)
        | ReviewRepositoryError::Json(_)
        | ReviewRepositoryError::Regex(_) => internal_error(error),
    }
}

fn internal_error(error: impl std::fmt::Display + std::fmt::Debug) -> HttpResponse {
    error!(error = ?error, "Review API error");
    HttpResponse::json(500, &json!({ "error": "internal server error" }))
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
    use std::net::SocketAddr;

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
                agent_token: None,
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
                agent_token: None,
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
                agent_token: None,
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
                agent_token: None,
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
        assert!(server.access.authorize(&reviews_request).is_some());

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
                agent_token: None,
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
    fn should_delete_a_schema_field_rule_only_on_an_explicit_null() {
        let deletion = serde_json::from_str::<SchemaFieldPayload>(
            r#"{"expected_candidate_version": 1, "schema_index": 0, "field": "price", "rule": null}"#,
        );

        assert!(matches!(
            deletion,
            Ok(SchemaFieldPayload { rule: None, .. })
        ));
        for body in [
            r#"{"expected_candidate_version": 1, "schema_index": 0, "field": "price"}"#,
            r#"{"expected_candidate_version": 1, "schema_index": 0, "field": "price", "rules": {"selector": ".price", "type": "text"}}"#,
        ] {
            assert!(
                serde_json::from_str::<SchemaFieldPayload>(body).is_err(),
                "{body} must be refused, not read as a deletion"
            );
        }
    }

    #[test]
    fn should_refuse_a_candidate_edit_that_names_no_candidate_version_it_was_based_on() {
        for refused in [
            serde_json::from_str::<SchemaFieldPayload>(
                r#"{"schema_index": 0, "field": "price", "rule": null}"#,
            )
            .err(),
            serde_json::from_str::<CandidatePayload>(r#"{"schemas": []}"#).err(),
        ] {
            assert!(
                refused
                    .is_some_and(|error| error.to_string().contains("expected_candidate_version")),
                "an edit without the version it was based on must be refused"
            );
        }

        let edit = serde_json::from_str::<CandidatePayload>(
            r#"{"expected_candidate_version": 3, "schemas": [], "removed_page_schemas": []}"#,
        )
        .unwrap_or_else(|error| panic!("versioned candidate edit: {error}"));
        assert_eq!(edit.expected_candidate_version, 3);
        assert_eq!(
            serde_json::Value::Object(edit.candidate),
            json!({ "schemas": [], "removed_page_schemas": [] }),
            "the version is no key of the candidate"
        );
    }

    mod matrix_refresh {
        use super::{ReviewServer, ReviewServerConfig};
        use crate::network::policy::NetworkErrorKind;
        use crate::review::matrix_refresh::MatrixRefreshStatus;
        use crate::review::model::{PAGE_ROLE_PRIMARY, SchemaReviewPageInput};
        use crate::review::repository::CrawlerReviewRepository;
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
            requested: AtomicUsize,
            fetched: AtomicUsize,
            failure: Option<&'static str>,
        }

        impl PacedFetcher {
            fn new(failure: Option<&'static str>) -> Arc<Self> {
                Arc::new(Self {
                    released: Semaphore::new(0),
                    requested: AtomicUsize::new(0),
                    fetched: AtomicUsize::new(0),
                    failure,
                })
            }

            fn release(&self, pages: usize) {
                self.released.add_permits(pages);
            }

            /// Waits until a refresh asks for its first page, by when it has read the review's
            /// page set.
            async fn wait_for_first_request(&self) {
                for _ in 0..200 {
                    if self.requested.load(Ordering::SeqCst) > 0 {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                panic!("the matrix refresh requested no page");
            }
        }

        #[async_trait::async_trait]
        impl HtmlFetcher for PacedFetcher {
            async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
                self.requested.fetch_add(1, Ordering::SeqCst);
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
            listing_source_id: ListingSourceId,
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
                        agent_token: None,
                        local_mutations_without_token: false,
                    },
                    fetcher.clone(),
                )
            };
            register_refresh_shutdown(server.clone(), fetcher, review_id);
            Fixture {
                pool,
                server,
                listing_source_id,
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

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_store_the_matrix_of_the_current_candidate_when_it_changes_during_the_refresh()
         {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;

            let started = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(started.starts_with("HTTP/1.1 202"), "{started}");
            let repository = CrawlerReviewRepository::new(fixture.pool.clone());
            let read_version = repository
                .get_review(fixture.review_id)
                .await
                .unwrap_or_else(|error| panic!("read review: {error}"))
                .review
                .candidate_version;
            repository
                .update_candidate_payload(
                    fixture.review_id,
                    read_version,
                    json!({ "schemas": [schema("h2")] }),
                )
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

        /// Waiting pages that join while the live pages are fetched must not fail the refresh,
        /// or a reviewer gets no matrix until the joins stop: it answers the matrix of the pages
        /// it evaluated, and the next refresh covers the page that joined.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_the_matrix_of_the_evaluated_pages_when_a_page_joins_during_the_refresh()
         {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;
            let repository = CrawlerReviewRepository::new(fixture.pool.clone());
            let joining_url = Url::parse("https://paced.example/products/joining")
                .unwrap_or_else(|error| panic!("joining page URL: {error}"));

            let started = get_matrix(&fixture.server, fixture.review_id, true).await;
            assert!(started.starts_with("HTTP/1.1 202"), "{started}");
            fetcher.wait_for_first_request().await;
            let joined = repository
                .join_pending_schema_review(&fixture.listing_source_id, &joining_url, PAGE_HTML)
                .await
                .unwrap_or_else(|error| panic!("join the pending review: {error}"));
            fetcher.release(PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, fixture.review_id).await;

            assert_eq!(joined, Some(fixture.review_id));
            assert_eq!(
                repository
                    .get_review_pages(fixture.review_id)
                    .await
                    .unwrap_or_else(|error| panic!("read review pages: {error}"))
                    .len(),
                PAGE_COUNT + 1
            );
            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            let stored = stored_matrix(&fixture.pool, fixture.review_id)
                .await
                .unwrap_or_else(|| panic!("the matrix of the evaluated pages must be stored"));
            let evaluated_urls: Vec<&str> = stored["candidates"][0]["pages"]
                .as_array()
                .unwrap_or_else(|| panic!("stored matrix has no pages: {stored}"))
                .iter()
                .filter_map(|page| page["url"].as_str())
                .collect();
            assert_eq!(evaluated_urls.len(), PAGE_COUNT, "{stored}");
            assert!(
                !evaluated_urls.contains(&joining_url.as_str()),
                "the joining page is left to the next refresh: {stored}"
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

        pub(super) const TOKEN: &str = "review-token";

        /// Sends an authorized request and returns its status line and JSON body.
        pub(super) async fn send(
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
                    agent_token: None,
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

    mod schema_editing {
        use super::matrix_refresh::schema;
        use super::needs_repair::{TOKEN, send};
        use super::{RejectingDomainAdministration, ReviewServer, ReviewServerConfig};
        use crate::CrawlerReviewId;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        /// A review server over a pending manual schema review, and that review.
        pub(super) async fn pending_review_server() -> (ReviewServer, CrawlerReviewId) {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
                 VALUES ($1, 'Edited source', 'edited-source', TRUE, NOW(), NOW())",
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
                    agent_token: None,
                    local_mutations_without_token: false,
                },
                Arc::new(MockHtmlFetcher::new()),
            );
            (server, review_id)
        }

        async fn candidate_version(server: &ReviewServer, review_id: CrawlerReviewId) -> i64 {
            let (_, detail) = send(server, &format!("GET /api/reviews/{review_id}"), "").await;
            detail["review"]["candidate_version"]
                .as_i64()
                .unwrap_or_else(|| panic!("review without candidate version: {detail}"))
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_edit_a_registered_raw_attribute_and_answer_with_the_candidate_version() {
            let (server, review_id) = pending_review_server().await;
            let schema_field = format!("POST /api/reviews/{review_id}/schema-field");
            let review = format!("GET /api/reviews/{review_id}");

            let (status, keys) = send(&server, "GET /api/raw-attribute-keys", "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {keys}");
            assert!(
                keys.as_array().is_some_and(|keys| keys
                    .iter()
                    .any(|key| { key["key"] == "rawMaterial" && key["group"] == "material" })),
                "{keys}"
            );
            let version = candidate_version(&server, review_id).await;

            let (status, edited) = send(
                &server,
                &schema_field,
                &json!({
                    "expected_candidate_version": version,
                    "schema_index": 0,
                    "field": "raw_attributes.rawMaterial",
                    "rule": {"selector": ".material", "type": "text"},
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {edited}");
            assert_eq!(edited["candidate_version"], json!(version + 1));
            let (_, after) = send(&server, &review, "").await;
            assert_eq!(after["review"]["candidate_version"], json!(version + 1));
            assert_eq!(
                after["review"]["candidate_payload"]["schemas"][0]["raw_attributes"]["rawMaterial"]
                    ["selector"],
                ".material"
            );
            assert_eq!(after["candidate_is_live"], false);

            let (status, refused) = send(
                &server,
                &schema_field,
                &json!({
                    "expected_candidate_version": version + 1,
                    "schema_index": 0,
                    "field": "raw_attributes.rawInvented",
                    "rule": {"selector": ".invented", "type": "text"},
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 400"), "{status} {refused}");
            assert!(
                refused["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("rawInvented")),
                "{refused}"
            );
        }

        /// Two writers, such as a reviewer and the review agent, edit the candidate version they
        /// both read at once: one edit applies, and the other is refused with 409 and the
        /// version it would have overwritten, instead of silently replacing the first.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_with_the_current_version_the_second_of_two_edits_based_on_one_version()
         {
            let (server, review_id) = pending_review_server().await;
            let version = candidate_version(&server, review_id).await;
            let field_edit = json!({
                "expected_candidate_version": version,
                "schema_index": 0,
                "field": "title",
                "rule": {"selector": "h1.field-edit", "type": "text"},
            })
            .to_string();
            let candidate_edit = json!({
                "expected_candidate_version": version,
                "schemas": [schema("h1.candidate-edit")],
            })
            .to_string();

            let schema_field = format!("POST /api/reviews/{review_id}/schema-field");
            let candidate = format!("POST /api/reviews/{review_id}/candidate");

            let (field_answer, candidate_answer) = tokio::join!(
                send(&server, &schema_field, &field_edit),
                send(&server, &candidate, &candidate_edit),
            );

            let ((applied, applied_title), (refused_status, refused)) =
                match (field_answer, candidate_answer) {
                    ((status, applied), refused) if status.starts_with("HTTP/1.1 200") => {
                        ((applied, "h1.field-edit"), refused)
                    }
                    (refused, (status, applied)) if status.starts_with("HTTP/1.1 200") => {
                        ((applied, "h1.candidate-edit"), refused)
                    }
                    answers => panic!("one of the edits must apply: {answers:?}"),
                };
            assert_eq!(applied["candidate_version"], json!(version + 1));
            assert!(
                refused_status.starts_with("HTTP/1.1 409"),
                "{refused_status} {refused}"
            );
            assert_eq!(refused["candidate_version"], json!(version + 1));
            assert!(refused["error"].is_string(), "{refused}");
            let (_, after) = send(&server, &format!("GET /api/reviews/{review_id}"), "").await;
            assert_eq!(after["review"]["candidate_version"], json!(version + 1));
            assert_eq!(
                after["review"]["candidate_payload"]["schemas"][0]["title"]["selector"],
                applied_title
            );
        }
    }

    /// The answers a client such as the review agent tells an unknown review and a closed
    /// review by.
    mod refused_review_requests {
        use super::matrix_refresh::schema;
        use super::needs_repair::send;
        use super::schema_editing::pending_review_server;
        use crate::review::matrix_refresh::MatrixRefreshStatus;
        use crate::{CrawlerReviewId, CrawlerReviewPageId};
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        /// Every review route refuses an unknown review with 404 and an `error` message, and a
        /// matrix refresh of it is refused before it starts, so it leaves no refresh behind.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_404_on_every_review_route_for_an_unknown_review() {
            let (server, _) = pending_review_server().await;
            let unknown = CrawlerReviewId::new();
            let schema_field = json!({
                "expected_candidate_version": 1,
                "schema_index": 0,
                "field": "title",
                "rule": {"selector": "h1", "type": "text"},
            });
            let candidate = json!({ "expected_candidate_version": 1, "schemas": [schema("h1")] });

            for (request, body) in [
                (format!("GET /api/reviews/{unknown}"), String::new()),
                (format!("GET /api/reviews/{unknown}/matrix"), String::new()),
                (
                    format!("GET /api/reviews/{unknown}/matrix?refresh=true"),
                    String::new(),
                ),
                (
                    format!("POST /api/reviews/{unknown}/approve"),
                    String::new(),
                ),
                (format!("POST /api/reviews/{unknown}/reject"), String::new()),
                (
                    format!("POST /api/reviews/{unknown}/needs-repair"),
                    json!({ "notes": "fix the title" }).to_string(),
                ),
                (
                    format!("POST /api/reviews/{unknown}/pages"),
                    json!({ "url": "https://paced.example/products/added" }).to_string(),
                ),
                (
                    format!("POST /api/reviews/{unknown}/schema-field"),
                    schema_field.to_string(),
                ),
                (
                    format!("POST /api/reviews/{unknown}/candidate"),
                    candidate.to_string(),
                ),
            ] {
                let (status, answer) = send(&server, &request, &body).await;
                assert!(
                    status.starts_with("HTTP/1.1 404"),
                    "{request}: {status} {answer}"
                );
                assert!(answer["error"].is_string(), "{request}: {answer}");
            }
            assert!(matches!(
                server.matrix_refreshes.status(unknown),
                MatrixRefreshStatus::Idle
            ));
        }

        /// The review-page routes refuse an unknown page with 404 and a malformed page id with
        /// 400, both with an `error` message like every other review route.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_json_errors_on_every_review_page_route_for_an_unknown_page() {
            let (server, _) = pending_review_server().await;
            let unknown = CrawlerReviewPageId::new();

            for (request, expected) in [
                (format!("DELETE /api/review-pages/{unknown}"), "404"),
                (format!("GET /api/review-pages/{unknown}/inspect"), "404"),
                (format!("GET /api/review-pages/{unknown}/html"), "404"),
                ("GET /api/review-pages/not-a-page/inspect".to_owned(), "400"),
                ("GET /api/review-pages/not-a-page/html".to_owned(), "400"),
            ] {
                let (status, answer) = send(&server, &request, "").await;
                assert!(
                    status.starts_with(&format!("HTTP/1.1 {expected}")),
                    "{request}: {status} {answer}"
                );
                assert!(answer["error"].is_string(), "{request}: {answer}");
            }
        }

        /// Rejecting a review that is no longer pending is a request the review cannot take, as
        /// a needs-repair verdict on it is, and is answered alike.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_rejecting_a_closed_review_with_400_like_needs_repair() {
            let (server, review_id) = pending_review_server().await;
            let reject = format!("POST /api/reviews/{review_id}/reject");
            let (status, rejected) = send(&server, &reject, "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {rejected}");

            let (reject_status, reject_again) = send(&server, &reject, "").await;
            let (repair_status, repair) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/needs-repair"),
                &json!({ "notes": "fix the title" }).to_string(),
            )
            .await;

            assert!(
                reject_status.starts_with("HTTP/1.1 400"),
                "{reject_status} {reject_again}"
            );
            assert!(
                repair_status.starts_with("HTTP/1.1 400"),
                "{repair_status} {repair}"
            );
            assert_eq!(reject_again, repair, "the same refusal in the same shape");
            assert!(
                reject_again["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("not pending")),
                "{reject_again}"
            );
        }
    }

    mod incomplete_coverage {
        use super::needs_repair::send;
        use super::schema_editing::pending_review_server;
        use crate::CrawlerReviewId;
        use crate::review::server::ReviewServer;
        use serde_json::json;
        use test_api::IntegrationTestService as _;
        use url::Url;

        async fn join_waiting_page(server: &ReviewServer, review_id: CrawlerReviewId, slug: &str) {
            let listing_source_id = server
                .repository
                .get_review(review_id)
                .await
                .unwrap_or_else(|error| panic!("read the review: {error}"))
                .review
                .listing_source_id;
            let waiting_url = Url::parse(&format!("https://edited.example/products/{slug}"))
                .unwrap_or_else(|error| panic!("waiting page URL: {error}"));
            server
                .repository
                .join_pending_schema_review(
                    &listing_source_id,
                    &waiting_url,
                    "<html><body><h1>Waiting</h1></body></html>",
                )
                .await
                .unwrap_or_else(|error| panic!("join the review: {error}"));
        }

        /// A client such as the review agent reads from the refusal how much of the review the
        /// candidate covers, and approves incomplete coverage only by acknowledging the review's
        /// current coverage: a page that joined since the acknowledged coverage was read refuses
        /// the approval again.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_approving_incomplete_coverage_until_its_current_coverage_is_acknowledged()
         {
            let (server, review_id) = pending_review_server().await;
            join_waiting_page(&server, review_id, "waiting").await;
            let approve = format!("POST /api/reviews/{review_id}/approve");
            let review = format!("GET /api/reviews/{review_id}");

            let (status, refused) = send(
                &server,
                &approve,
                &json!({ "notes": "looks right" }).to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");
            assert_eq!(
                refused["coverage"],
                json!({ "covered": 0, "removed": 0, "total": 1, "not_evaluated": 1 })
            );
            assert!(refused["error"].is_string(), "{refused}");

            join_waiting_page(&server, review_id, "joined-later").await;
            let (status, stale) = send(
                &server,
                &approve,
                &json!({ "notes": "looks right", "acknowledged_coverage": refused["coverage"] })
                    .to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 409"), "{status} {stale}");
            assert_eq!(
                stale["coverage"],
                json!({ "covered": 0, "removed": 0, "total": 2, "not_evaluated": 2 })
            );
            let (_, pending) = send(&server, &review, "").await;
            assert_eq!(pending["review"]["status"], "PENDING_REVIEW");

            let (status, approved) = send(
                &server,
                &approve,
                &json!({ "notes": "looks right", "acknowledged_coverage": stale["coverage"] })
                    .to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 200"), "{status} {approved}");
            let (_, decided) = send(&server, &review, "").await;
            assert_eq!(decided["review"]["status"], "APPROVED");
        }
    }
}
