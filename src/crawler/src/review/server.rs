use crate::network::policy::NetworkErrorKind;
use crate::review::access::ReviewAccess;
pub use crate::review::access::{ReviewServerConfig, ReviewServerConfigError};
use crate::review::agent_decisions::AGENT_LIMITS_ROUTE;
use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::candidate_evaluations::CandidateEvaluations;
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::live_jobs::{LiveJobLimit, LiveJobsBusy};
use crate::review::matrix_refresh::MatrixRefreshes;
use crate::review::model::{
    CrawlerReviewPage, RawAttributeKey, ReviewActor, ReviewDetail, SampleLabelInput,
};
use crate::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crate::review::readers::{ListingSourceOverviewReader, ReviewQueueReader};
use crate::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
use crate::spider::local_lock::LocalLockManager;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::ListingSourceId;
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
/// How long a refresh request waits for the matrix before answering that it is still
/// refreshing, leaving room within [`REQUEST_DEADLINE`].
const MATRIX_REFRESH_RESPONSE_WAIT: Duration = Duration::from_secs(20);
const RESPONSE_WRITE_DEADLINE: Duration = Duration::from_secs(10);
/// The most events one answer of `GET /api/review-events` holds; a client reads on from the last
/// event's time, as the review agents' daily digest does.
const MAX_LISTED_REVIEW_EVENTS: usize = 1_000;

mod mass_removal_confirmation;
mod missing_value_pages;
mod pending_reviews;
mod schema_matrix;
mod source_pages;

use source_pages::{EvaluationPayload, parse_review_evaluation_id};

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    review_queue: ReviewQueueReader,
    listing_sources: ListingSourceOverviewReader,
    access: ReviewAccess,
    html_fetcher: Arc<dyn HtmlFetcher>,
    /// The console's cap on live matrix refreshes, candidate evaluations and page projections.
    live_jobs: LiveJobLimit,
    matrix_refreshes: MatrixRefreshes,
    matrix_refresh_wait: Duration,
    candidate_evaluations: CandidateEvaluations,
}

impl ReviewServer {
    /// `html_fetcher` serves live inspection; pass the scraper's fetcher so
    /// both count against the same robots.txt cache and per-domain pacing. `repository` carries
    /// the review agents' decision policy the composition root built for the whole crawler.
    pub fn new(
        repository: CrawlerReviewRepository,
        config: ReviewServerConfig,
        html_fetcher: Arc<dyn HtmlFetcher>,
    ) -> Self {
        Self {
            review_queue: ReviewQueueReader::of(&repository),
            listing_sources: ListingSourceOverviewReader::new(repository.pool().clone()),
            repository,
            live_jobs: LiveJobLimit::new(config.max_concurrent_live_jobs),
            access: ReviewAccess::new(config),
            html_fetcher,
            matrix_refreshes: MatrixRefreshes::default(),
            matrix_refresh_wait: MATRIX_REFRESH_RESPONSE_WAIT,
            candidate_evaluations: CandidateEvaluations::default(),
        }
    }

    /// This console reporting, in the ListingSource overview, which sources the crawler of this
    /// process is crawling or scraping right now from `work_locks`, its lock manager.
    pub fn with_work_locks(self, work_locks: Arc<LocalLockManager>) -> Self {
        Self {
            listing_sources: self.listing_sources.with_work_locks(work_locks),
            ..self
        }
    }

    /// Serves the console on its configured bind address.
    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListener::bind(self.access.bind_addr()).await?;
        self.serve(listener).await
    }

    /// Serves the console on `listener`, such as one bound to an ephemeral port in a test.
    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        let bind_addr = listener.local_addr()?;
        let agent_limits = *self.repository.agent_decisions().limits();
        info!(
            bind_addr = %bind_addr,
            agent_access = self.access.agent_access(),
            agent_max_decisions_per_hour = agent_limits.max_decisions_per_hour.get(),
            agent_matrix_max_age_minutes = agent_limits.matrix_max_age_minutes.get(),
            agent_min_labelled_urls = agent_limits.min_labelled_urls.get(),
            agent_max_repair_rounds = agent_limits.max_repair_rounds.get(),
            max_concurrent_live_jobs = self.live_jobs.limit().get(),
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

        let Some(actor) = self.access.authorize(&parsed) else {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        };

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("POST", "/api/session") => self.access.create_session(),
            ("POST", "/api/session/logout") => self.access.clear_session(&parsed),
            ("GET", "/api/listing_sources") => {
                match self.listing_sources.list_listing_sources(200).await {
                    Ok(listing_sources) => HttpResponse::json(200, &listing_sources),
                    Err(err) => internal_error(err),
                }
            }
            ("GET", "/api/raw-attribute-keys") => {
                HttpResponse::json(200, &RawAttributeKey::registry())
            }
            ("GET", AGENT_LIMITS_ROUTE) => {
                HttpResponse::json(200, self.repository.agent_decisions().limits())
            }
            ("GET", "/api/reviews") => match self.review_queue.list_reviews(200).await {
                Ok(reviews) => HttpResponse::json(200, &reviews),
                Err(err) => internal_error(err),
            },
            ("GET", "/api/pending-reviews") => self.pending_reviews(&parsed).await,
            ("GET", "/api/review-events") => self.review_events_since(&parsed).await,
            _ => self.route_dynamic(parsed, actor).await,
        }
    }

    /// `GET /api/review-events?since=<RFC 3339 time>`: the events of all reviews since then,
    /// oldest first and without their details, and whether later ones were left out.
    async fn review_events_since(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let Some(since) = request.query.get("since").and_then(|since| {
            time::OffsetDateTime::parse(since, &time::format_description::well_known::Rfc3339).ok()
        }) else {
            return HttpResponse::json(
                400,
                &json!({ "error": "since must be an RFC 3339 time, such as 2026-10-28T06:00:00Z" }),
            );
        };
        match self
            .repository
            .list_review_events_since(since, MAX_LISTED_REVIEW_EVENTS)
            .await
        {
            Ok((events, truncated)) => {
                HttpResponse::json(200, &json!({ "events": events, "truncated": truncated }))
            }
            Err(err) => internal_error(err),
        }
    }

    /// Routes a request with a path parameter. Review mutations are recorded as `actor`'s.
    async fn route_dynamic(&self, request: ParsedRequest<'_>, actor: ReviewActor) -> HttpResponse {
        if let Some(response) = self.route_missing_value_pages(&request, actor).await {
            return response;
        }
        if let Some(response) = self.route_mass_removal_confirmation(&request, actor).await {
            return response;
        }
        // Preview responses, including their errors, are framed by the console.
        if request.method == "GET" && request.path == "/api/live-inspect" {
            return match live_preview_url(&request) {
                Ok(url) => match self.fetch_live_url(&url).await {
                    Ok(html) => HttpResponse::html(200, &instrument_live_html(url.as_str(), &html)),
                    Err(response) => response,
                },
                Err(response) => response,
            }
            .into_review_preview();
        }

        if request.method == "GET" && request.path == "/api/live-html" {
            return match live_preview_url(&request) {
                Ok(url) => match self.fetch_live_url(&url).await {
                    Ok(html) => HttpResponse::text(200, &html),
                    Err(response) => response,
                },
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/page-projection")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/page-projection")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            return match self
                .project_review_source_page(review_id, actor, &request)
                .await
            {
                Ok(projection) => HttpResponse::json(200, &projection),
                Err(response) => response,
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/evaluate")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/evaluate") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: EvaluationPayload = match serde_json::from_str(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return self.evaluate_candidate(review_id, actor, payload).await;
        }

        if request.method == "GET"
            && request.path.starts_with("/api/reviews/")
            && request.path.contains("/evaluations/")
        {
            let Some((review_id, evaluation_id)) = parse_review_evaluation_id(request.path) else {
                return HttpResponse::json(
                    400,
                    &json!({ "error": "invalid review or evaluation id" }),
                );
            };
            return self.candidate_evaluation(review_id, evaluation_id).await;
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
            return if refresh {
                self.refresh_schema_matrix(review_id, actor).await
            } else {
                self.read_schema_matrix(review_id, actor).await
            };
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
            return match self.add_reviewer_page(review_id, actor, url).await {
                Ok(page) => HttpResponse::json(201, &page),
                Err(response) => response,
            };
        }

        if request.method == "DELETE" && request.path.starts_with("/api/review-pages/") {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "") else {
                return HttpResponse::json(400, &json!({ "error": "invalid page id" }));
            };
            return match self.repository.remove_reviewer_page(page_id, actor).await {
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
            if let Err(error) = self.repository.refresh_url_pattern_sample(review_id).await {
                warn!(review_id = %review_id, error = %error, "could not refresh the URL sample of the review");
            }
            return match self.repository.get_review(review_id).await {
                Ok(detail) => {
                    log_invalid_candidate(&detail);
                    HttpResponse::json(200, &detail)
                }
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/approve")
        {
            let (review_id, payload) =
                match verdict_request::<ApprovalPayload>(&request, "/approve") {
                    Ok(verdict) => verdict,
                    Err(response) => return response,
                };
            return verdict_answer(
                self.repository
                    .approve_review(review_id, actor, payload.approval())
                    .await,
            );
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/restore")
        {
            let (review_id, payload) = match verdict_request::<ActionPayload>(&request, "/restore")
            {
                Ok(verdict) => verdict,
                Err(response) => return response,
            };
            return verdict_answer(
                self.repository
                    .restore_approval(review_id, actor, payload.notes.as_deref())
                    .await,
            );
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/revoke-rejection")
        {
            let (review_id, payload) =
                match verdict_request::<ActionPayload>(&request, "/revoke-rejection") {
                    Ok(verdict) => verdict,
                    Err(response) => return response,
                };
            return verdict_answer(
                self.repository
                    .revoke_rejection(review_id, actor, payload.notes.as_deref())
                    .await,
            );
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/reject")
        {
            let (review_id, payload) = match verdict_request::<ActionPayload>(&request, "/reject") {
                Ok(verdict) => verdict,
                Err(response) => return response,
            };
            return verdict_answer(
                self.repository
                    .reject_review(review_id, actor, payload.notes.as_deref())
                    .await,
            );
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/needs-repair")
        {
            let (review_id, payload) =
                match verdict_request::<ActionPayload>(&request, "/needs-repair") {
                    Ok(verdict) => verdict,
                    Err(response) => return response,
                };
            return verdict_answer(
                self.repository
                    .request_repair(review_id, actor, payload.notes.as_deref())
                    .await,
            );
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
                    actor,
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
            && request.path.ends_with("/sample-labels")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/sample-labels")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: SampleLabelsPayload = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .set_sample_labels(
                    review_id,
                    actor,
                    payload.expected_candidate_version,
                    payload.labels,
                )
                .await
            {
                Ok(candidate_version) => candidate_edited(candidate_version),
                Err(err) => review_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/no-sold-page")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/no-sold-page") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: NoSoldPagePayload = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .record_no_sold_page(review_id, actor, payload.urls, &payload.notes)
                .await
            {
                Ok(recorded_urls) => {
                    HttpResponse::json(200, &json!({ "ok": true, "recorded_urls": recorded_urls }))
                }
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
                    actor,
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

    /// Fetches a reviewer-chosen page of the review's ListingSource and stores it as a review
    /// page. The target is checked before the fetch so the console never fetches a foreign
    /// host, and the final URL after redirects is checked again before it is stored.
    async fn add_reviewer_page(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        url: Url,
    ) -> Result<CrawlerReviewPage, HttpResponse> {
        self.repository
            .check_reviewer_page_target(review_id, actor, &url)
            .await
            .map_err(review_error)?;
        let fetched = self.fetch_reviewer_page(&url).await?;
        self.repository
            .add_reviewer_page(review_id, actor, &url, &fetched.final_url, &fetched.html)
            .await
            .map_err(review_error)
    }

    /// Fetches a page a reviewer adds to a review, after its target was checked.
    async fn fetch_reviewer_page(&self, url: &Url) -> Result<FetchedHtml, HttpResponse> {
        self.html_fetcher
            .fetch(url)
            .await
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))
    }

    /// Fetches the review page live for a human's preview (`/api/review-pages/{id}/inspect`,
    /// `/html`), outside the console's cap on live jobs; the console refuses the review agents'
    /// tokens on these routes (see [`ReviewAccess::authorize`]).
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

    async fn fetch_live_url(&self, url: &Url) -> Result<String, HttpResponse> {
        self.fetch_live(url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))
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

#[derive(Deserialize)]
struct ActionPayload {
    notes: Option<String>,
}

/// An approval of `POST /api/reviews/{id}/approve`. `expected_candidate_version` is the
/// candidate version the approver read, as for a candidate edit. `acknowledged_coverage` approves
/// a product-schema candidate that does not cover every review page while the review's coverage
/// is still the one it names; the console sends the coverage its confirmation dialog stated.
#[derive(Deserialize)]
struct ApprovalPayload {
    expected_candidate_version: i64,
    notes: Option<String>,
    acknowledged_coverage: Option<ReviewPageCoverage>,
}

impl ApprovalPayload {
    fn approval(&self) -> Approval<'_> {
        Approval {
            expected_candidate_version: self.expected_candidate_version,
            notes: self.notes.as_deref(),
            incomplete_coverage: self
                .acknowledged_coverage
                .map_or(IncompleteCoverage::Refuse, IncompleteCoverage::Acknowledged),
        }
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

/// The sample labels of `POST /api/reviews/{id}/sample-labels`, which replace the URL-pattern
/// candidate's labels as a whole, next to the candidate version they were based on.
#[derive(Deserialize)]
struct SampleLabelsPayload {
    expected_candidate_version: i64,
    labels: Vec<SampleLabelInput>,
}

/// `POST /api/reviews/{id}/no-sold-page`: the pages the author checked and its account of how.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoSoldPagePayload {
    urls: Vec<String>,
    notes: String,
}

#[derive(Deserialize)]
struct ReviewPagePayload {
    url: String,
}

/// Reads the body of a verdict, where an empty body stands for an empty object: a verdict
/// without notes, or an approval refused for naming no candidate version.
fn parse_verdict_payload<T: DeserializeOwned>(body: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(if body.trim().is_empty() { "{}" } else { body })
}

/// The review id and body of a `POST /api/reviews/{id}{suffix}` verdict, or the 400 a path
/// without a review id or a body that does not parse is answered with.
fn verdict_request<T: DeserializeOwned>(
    request: &ParsedRequest<'_>,
    suffix: &str,
) -> Result<(CrawlerReviewId, T), HttpResponse> {
    let review_id = parse_review_id_with_suffix(request.path, suffix)
        .ok_or_else(|| HttpResponse::json(400, &json!({ "error": "invalid review id" })))?;
    let payload = parse_verdict_payload(request.body)
        .map_err(|error| HttpResponse::json(400, &json!({ "error": error.to_string() })))?;
    Ok((review_id, payload))
}

/// Answers a verdict that was taken with `{"ok": true}`, and a refused or failed one as
/// [`review_error`] does.
fn verdict_answer(taken: Result<(), ReviewRepositoryError>) -> HttpResponse {
    match taken {
        Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
        Err(error) => review_error(error),
    }
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

fn parse_page_id_with_suffix(path: &str, suffix: &str) -> Option<CrawlerReviewPageId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/review-pages/")?;
    id.parse().ok()
}

/// The URL a live preview (`/api/live-inspect`, `/api/live-html`) fetches: any URL its `url` query
/// names. Only humans reach the live previews: the console refuses the review agents' tokens on
/// them (see [`ReviewAccess::authorize`]).
fn live_preview_url(request: &ParsedRequest<'_>) -> Result<Url, HttpResponse> {
    let Some(raw_url) = request.query.get("url") else {
        return Err(HttpResponse::json(400, &json!({ "error": "missing url" })));
    };
    Url::parse(raw_url).map_err(|err| {
        HttpResponse::json(
            400,
            &json!({
                "error": "invalid live preview url",
                "url": raw_url,
                "details": err.to_string(),
            }),
        )
    })
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
/// unknown review, review page or ListingSource, 400 for a request the review cannot take, 403 with a `reason`
/// for a change the actor may not make, 409 for a request the review's current state refuses,
/// a URL-pattern review whose ListingSource has no crawl root among them,
/// 429 with `Retry-After` once the review agent made its decisions of the hour, and 500 for a
/// technical failure. A 409 for a request based on an older candidate carries the review's
/// current `candidate_version`, one for an approval of incomplete coverage, unacknowledged or
/// acknowledged at other counts, carries the review's current `coverage`, one for an agent
/// approval the approval guard refuses lists its `failed_checks`, and one for an agent request on
/// a review that waits for a human carries `reason` `HUMAN_REQUIRED`, the agents' `agent_rounds`
/// and the cap as `limit`.
fn review_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::NotFound(_)
        | ReviewRepositoryError::ReviewPageNotFound(_)
        | ReviewRepositoryError::ListingSourceNotFound(_) => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::NotPending(_)
        | ReviewRepositoryError::MissingRepairNote
        | ReviewRepositoryError::MissingRejectionNote
        | ReviewRepositoryError::UnsupportedArtifact(_, _)
        | ReviewRepositoryError::InvalidSchemaField(_)
        | ReviewRepositoryError::RequiredSchemaField(_)
        | ReviewRepositoryError::UnregisteredRawAttributeKey(_)
        | ReviewRepositoryError::InvalidUrlPatternCandidate
        | ReviewRepositoryError::InvalidProductSchemaCandidate
        | ReviewRepositoryError::RefusedProductSchemaCandidate(_)
        | ReviewRepositoryError::InvalidRemovedPageTemplate { .. }
        | ReviewRepositoryError::ReviewPageUrlNotAllowed(_)
        | ReviewRepositoryError::TooManyEvaluatedUrls
        | ReviewRepositoryError::ReviewPageLimitReached(_)
        | ReviewRepositoryError::NotReviewerAddedPage(_)
        | ReviewRepositoryError::RedirectedToNonProductPage(_)
        | ReviewRepositoryError::NoApprovalToRestore(_)
        | ReviewRepositoryError::ApprovalNotRestorable { .. }
        | ReviewRepositoryError::NoRejectionToRevoke(_)
        | ReviewRepositoryError::NoSoldPageUrlCount
        | ReviewRepositoryError::InvalidNoSoldPageNote
        | ReviewRepositoryError::TooManySampleLabels
        | ReviewRepositoryError::SampleLabelUrlNotAllowed(_)
        | ReviewRepositoryError::InvalidSampleLabelReason(_)
        | ReviewRepositoryError::DuplicateSampleLabel(_)
        | ReviewRepositoryError::SampleLabelsChangedThroughCandidate => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(_) => {
            forbidden(&error, "AGENT_CHANGES_ONLY_PENDING_REVIEWS")
        }
        ReviewRepositoryError::ReviewerMayNotChangeCandidates => {
            forbidden(&error, "REVIEWER_CANNOT_CHANGE_CANDIDATES")
        }
        ReviewRepositoryError::AuthorMayNotDecide => forbidden(&error, "AUTHOR_CANNOT_DECIDE"),
        ReviewRepositoryError::AgentCoverageAcknowledgement => {
            forbidden(&error, "AGENT_COVERAGE_ACKNOWLEDGEMENT")
        }
        ReviewRepositoryError::RestoreNeedsHuman => forbidden(&error, "RESTORE_NEEDS_HUMAN"),
        ReviewRepositoryError::RevocationNeedsHuman => forbidden(&error, "REVOKE_NEEDS_HUMAN"),
        ReviewRepositoryError::StaleCandidateVersion { current, .. } => {
            review_changed(&error, current)
        }
        ReviewRepositoryError::CandidateChangedDuringEvaluation { candidate_version } => {
            review_changed(&error, candidate_version)
        }
        ReviewRepositoryError::StaleApprovedReview(_)
        | ReviewRepositoryError::NotMissingValuePage(_)
        | ReviewRepositoryError::NoLiveProductSchemas(_)
        | ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(_)
        | ReviewRepositoryError::DuplicateReviewPage(_)
        | ReviewRepositoryError::ApprovalAlreadyRestored(_)
        | ReviewRepositoryError::RejectionAlreadyRevoked(_)
        | ReviewRepositoryError::LiveConfigurationChangedSinceApproval(_)
        | ReviewRepositoryError::UrlPatternReviewSourceNotCrawled(_) => {
            HttpResponse::json(409, &json!({ "error": error.to_string() }))
        }
        ReviewRepositoryError::IncompleteCoverage { coverage, .. }
        | ReviewRepositoryError::StaleCoverageAcknowledgement { coverage, .. } => {
            HttpResponse::json(
                409,
                &json!({ "error": error.to_string(), "coverage": coverage }),
            )
        }
        ReviewRepositoryError::HumanRequired {
            agent_rounds,
            max_repair_rounds,
            ..
        } => HttpResponse::json(
            409,
            &json!({
                "error": error.to_string(),
                "reason": "HUMAN_REQUIRED",
                "agent_rounds": agent_rounds,
                "limit": max_repair_rounds,
            }),
        ),
        ReviewRepositoryError::AgentApprovalRefused {
            ref failed_checks, ..
        } => HttpResponse::json(
            409,
            &json!({ "error": error.to_string(), "failed_checks": failed_checks }),
        ),
        ReviewRepositoryError::AgentDecisionLimitReached { limit, retry_after } => {
            HttpResponse::json(
                429,
                &json!({
                    "error": error.to_string(),
                    "reason": "AGENT_DECISION_LIMIT",
                    "limit": limit,
                    "retry_after_secs": retry_after.as_secs(),
                }),
            )
            .with_header("Retry-After", retry_after.as_secs().to_string())
        }
        ReviewRepositoryError::Database(_)
        | ReviewRepositoryError::Json(_)
        | ReviewRepositoryError::Regex(_)
        | ReviewRepositoryError::MatrixEvaluationStopped { .. } => internal_error(error),
    }
}

/// Logs at ERROR, without the candidate itself, why the loaded review's persisted candidate fails
/// validation. Only the review detail a reviewer loads logs it; the matrix and other reads of the
/// same review refuse or flag the candidate without logging it again.
fn log_invalid_candidate(detail: &ReviewDetail) {
    if let Some(failure) = &detail.candidate_error {
        error!(
            review_id = %detail.review.review_id,
            artifact_type = %detail.review.artifact_type,
            status = %detail.review.status,
            failure = %failure,
            "Persisted review candidate fails validation"
        );
    }
}

/// Answers a change `error` says the actor may not make with 403 and the machine-readable
/// `reason` a client such as the review agent acts on.
fn forbidden(error: &ReviewRepositoryError, reason: &str) -> HttpResponse {
    HttpResponse::json(
        403,
        &json!({ "error": error.to_string(), "reason": reason }),
    )
}

/// Answers a live job (a matrix refresh, a candidate evaluation or a page projection) the console
/// does not start because it runs as many as its limit allows: 429 with `reason` `LIVE_JOBS_BUSY`,
/// the limit and when to retry, also as `Retry-After`.
fn live_jobs_busy(busy: LiveJobsBusy) -> HttpResponse {
    let retry_after_secs = busy.retry_after.as_secs();
    HttpResponse::json(
        429,
        &json!({
            "error": format!(
                "the review console runs {} live jobs (matrix refreshes, candidate evaluations and page projections) at once, as many as it allows; retry in {retry_after_secs} seconds",
                busy.limit
            ),
            "reason": "LIVE_JOBS_BUSY",
            "limit": busy.limit.get(),
            "retry_after_secs": retry_after_secs,
        }),
    )
    .with_header("Retry-After", retry_after_secs.to_string())
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
    live_fetch_failure(&page.url, "failed to fetch live review HTML", error)
}

fn live_url_fetch_error(url: &str, error: &FetchError) -> HttpResponse {
    warn!(
        url,
        error = ?error,
        "Failed to fetch live preview HTML"
    );
    live_fetch_failure(url, "failed to fetch live preview HTML", error)
}

/// The `502` a failed live fetch of `url` answers. A bot challenge the shop answered instead of
/// the page says so and carries `reason` `BOT_CHALLENGE`, so a reviewer does not read it as the
/// page failing a schema.
fn live_fetch_failure(url: &str, failed: &str, error: &FetchError) -> HttpResponse {
    let body = if error.kind() == NetworkErrorKind::BotChallenge {
        json!({
            "error": format!("the shop answered {url} with a bot challenge instead of the page; retry later"),
            "reason": "BOT_CHALLENGE",
            "url": url,
            "details": error.to_string(),
        })
    } else {
        json!({
            "error": failed,
            "url": url,
            "details": error.to_string(),
        })
    };
    HttpResponse::json(502, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authoring::CrawlerAuthoringMode;
    use crate::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
    use crate::review::agent_tokens::ReviewAgentTokens;
    use crate::review::approval_guard::FailedApprovalCheck;
    use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
    use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
    use std::net::SocketAddr;

    mod live_fetch_failure {
        use super::*;

        fn body_of(response: HttpResponse) -> (String, serde_json::Value) {
            let response = response.to_string();
            let (head, body) = response.split_once("\r\n\r\n").unwrap();
            (
                head.lines().next().unwrap().to_owned(),
                serde_json::from_str(body).unwrap(),
            )
        }

        #[test]
        fn should_name_a_bot_challenge_the_shop_answered_instead_of_the_page() {
            let error = FetchError::NetworkWithMetadata {
                kind: NetworkErrorKind::BotChallenge,
                status: Some(403),
                retry_after: None,
                details: "bot challenge instead of the page".to_owned(),
            };

            let (status, body) = body_of(live_url_fetch_error("https://shop.example/p/1", &error));

            assert!(status.starts_with("HTTP/1.1 502"), "{status}");
            assert_eq!(body["reason"], "BOT_CHALLENGE");
            assert_eq!(body["url"], "https://shop.example/p/1");
            assert!(body["error"].as_str().unwrap().contains("bot challenge"));
        }

        #[test]
        fn should_keep_the_answer_of_any_other_fetch_failure() {
            let error = FetchError::Network {
                kind: NetworkErrorKind::Connect,
                details: "connection refused".to_owned(),
            };

            let (_, body) = body_of(live_url_fetch_error("https://shop.example/p/1", &error));

            assert_eq!(body["error"], "failed to fetch live preview HTML");
            assert!(body.get("reason").is_none());
        }
    }

    /// The review agents' decision policy of `agent` authoring mode with `limits`, as the crawler
    /// builds it for a console open to the agents.
    fn agent_mode(limits: AgentLimits) -> AgentDecisionPolicy {
        AgentDecisionPolicy::new(CrawlerAuthoringMode::Agent, limits)
    }

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
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: auth_token.map(str::to_owned),
                agent_tokens: None,
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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

    /// The console answers the limits of the agent decision policy its repository applies to
    /// whoever may read it, a human and the author agent whose token the review agent runner
    /// reads them with, so the runner tells the agents those limits.
    #[tokio::test]
    async fn should_answer_the_agents_limits_the_console_applies()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let limits = AgentLimits {
            min_labelled_urls: std::num::NonZeroU32::new(5).ok_or("five is not zero")?,
            ..AgentLimits::default()
        };
        let server = ReviewServer::new(
            CrawlerReviewRepository::new(pool).with_agent_decisions(agent_mode(limits)),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: Some("review-token".to_owned()),
                agent_tokens: Some(ReviewAgentTokens::of("author-token", "reviewer-token")),
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
            },
            Arc::new(MockHtmlFetcher::new()),
        );
        let read = |authorization: &str| {
            format!("GET {AGENT_LIMITS_ROUTE} HTTP/1.1\r\nHost: 127.0.0.1\r\n{authorization}\r\n")
        };

        for token in ["review-token", "author-token"] {
            let answered = server
                .route(&read(&format!("Authorization: Bearer {token}\r\n")))
                .await
                .to_string();

            assert!(answered.starts_with("HTTP/1.1 200"), "{token}: {answered}");
            let body = answered
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .ok_or("answer without body")?;
            assert_eq!(
                serde_json::from_str::<AgentLimits>(body)?,
                limits,
                "{token}"
            );
        }
        let refused = server.route(&read("")).await.to_string();
        assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");
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
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                agent_tokens: None,
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                agent_tokens: None,
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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

        let live_html = server
            .route("GET /api/live-html?url=https%3A%2F%2Fexample.com HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let page_html = server
            .route("GET /api/review-pages/not-a-page/html HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(inspection.starts_with("HTTP/1.1 200 OK"));
        for preview in [&inspection, &missing_url, &invalid_page] {
            assert!(preview.contains("Content-Security-Policy: sandbox allow-scripts;"));
            assert!(preview.contains("img-src https: data:;"));
            assert!(preview.contains("base-uri https:;"));
            assert!(preview.contains("frame-ancestors 'self'\r\n"));
            assert!(preview.contains("\r\nReferrer-Policy: no-referrer\r\n"));
        }
        for strict in [&console, &live_html, &page_html] {
            assert!(!strict.contains("sandbox"));
            assert!(strict.contains("default-src 'self';"));
            assert!(strict.contains("base-uri 'none';"));
            assert!(strict.contains("frame-ancestors 'none'\r\n"));
        }
        Ok(())
    }

    /// The live previews of any URL fetch it for a human outside the console's cap on live jobs, as
    /// the browser console always did. They and the live previews of a review page refuse both
    /// review agents' tokens with `401` before reading the review or page they name, so no agent
    /// fetches a page through them, at the cap or on a review that waits for a human.
    #[tokio::test]
    async fn should_fetch_live_previews_for_humans_outside_the_live_job_cap_and_for_no_agent()
    -> Result<(), Box<dyn std::error::Error>> {
        use needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN};

        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let mut human_fetches = MockHtmlFetcher::new();
        human_fetches.expect_fetch().times(2).returning(|url| {
            let final_url = url.clone();
            Box::pin(async move {
                Ok(FetchedHtml {
                    html: "<h1>Chair</h1>".to_owned(),
                    final_url,
                    status: reqwest::StatusCode::OK,
                })
            })
        });
        let server = ReviewServer::new(
            CrawlerReviewRepository::new(pool),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: Some(TOKEN.to_owned()),
                agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                local_mutations_without_token: false,
                max_concurrent_live_jobs: std::num::NonZeroU32::MIN,
            },
            Arc::new(human_fetches),
        );
        let _only_place = server
            .live_jobs
            .try_start()
            .unwrap_or_else(|busy| panic!("the console's one live job place: {busy:?}"));
        let query = format!(
            "url=https%3A%2F%2Fshop.example%2Fitem%2Fchair&review={}",
            CrawlerReviewId::new()
        );

        for route in ["/api/live-inspect", "/api/live-html"] {
            let request = |token: &str| {
                format!("GET {route}?{query} HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n")
            };
            for agent in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                let refused = server.route(&request(agent)).await.to_string();
                assert!(refused.starts_with("HTTP/1.1 401"), "{route}: {refused}");
            }
            let fetched = server.route(&request(TOKEN)).await.to_string();
            assert!(fetched.starts_with("HTTP/1.1 200"), "{route}: {fetched}");
        }
        let page_id = CrawlerReviewPageId::new();
        for route in ["inspect", "html"] {
            for agent in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                let refused = server
                    .route(&format!(
                        "GET /api/review-pages/{page_id}/{route} HTTP/1.1\r\nAuthorization: Bearer {agent}\r\n\r\n"
                    ))
                    .await
                    .to_string();
                assert!(refused.starts_with("HTTP/1.1 401"), "{route}: {refused}");
            }
        }
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
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: Some("review-token".to_string()),
                agent_tokens: None,
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                agent_tokens: None,
                local_mutations_without_token: false,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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

    /// A URL-pattern review whose ListingSource has no crawl root answers 409 naming the review,
    /// never the 404 of an unknown review or the 500 of a database failure.
    #[test]
    fn should_answer_a_url_pattern_review_of_an_uncrawled_source_with_a_conflict() {
        let review_id = CrawlerReviewId::new();

        let response = review_error(ReviewRepositoryError::UrlPatternReviewSourceNotCrawled(
            review_id,
        ))
        .to_string();

        assert!(response.starts_with("HTTP/1.1 409 Conflict"), "{response}");
        assert!(response.contains(&review_id.to_string()), "{response}");
        assert!(response.contains("no crawl root"), "{response}");
    }

    /// The answers a client such as the review agent acts on: a 403 names its `reason`, a guard
    /// refusal lists its `failed_checks`, the hourly cap says when to retry, and a review that
    /// waits for a human says so with the agents' rounds and the cap.
    #[test]
    fn should_answer_agent_refusals_with_a_reason_failed_checks_or_a_retry_time() {
        let answer = |error| {
            let response = review_error(error).to_string();
            let (head, body) = response
                .split_once("\r\n\r\n")
                .unwrap_or_else(|| panic!("response without body: {response}"));
            let body: serde_json::Value = serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("body is not JSON ({error}): {response}"));
            (head.to_owned(), body)
        };

        for (error, reason) in [
            (
                ReviewRepositoryError::AgentCoverageAcknowledgement,
                "AGENT_COVERAGE_ACKNOWLEDGEMENT",
            ),
            (
                ReviewRepositoryError::RestoreNeedsHuman,
                "RESTORE_NEEDS_HUMAN",
            ),
            (
                ReviewRepositoryError::RevocationNeedsHuman,
                "REVOKE_NEEDS_HUMAN",
            ),
            (
                ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(CrawlerReviewId::new()),
                "AGENT_CHANGES_ONLY_PENDING_REVIEWS",
            ),
        ] {
            let (head, body) = answer(error);
            assert!(head.starts_with("HTTP/1.1 403 Forbidden"), "{head}");
            assert_eq!(body["reason"], reason);
            assert!(body["error"].is_string(), "{body}");
        }

        let (head, body) = answer(ReviewRepositoryError::AgentApprovalRefused {
            review_id: CrawlerReviewId::new(),
            failed_checks: vec![FailedApprovalCheck::LiveMatrixMissing],
        });
        assert!(head.starts_with("HTTP/1.1 409 Conflict"), "{head}");
        assert_eq!(
            body["failed_checks"],
            json!([{ "check": "LIVE_MATRIX_MISSING" }])
        );
        assert!(
            body["error"].as_str().is_some_and(
                |error| error.contains("no schema matrix evaluated on the live review pages")
            ),
            "{body}"
        );

        let (head, body) = answer(ReviewRepositoryError::AgentDecisionLimitReached {
            limit: 20,
            retry_after: Duration::from_secs(754),
        });
        assert!(head.starts_with("HTTP/1.1 429 Too Many Requests"), "{head}");
        assert!(head.contains("\r\nRetry-After: 754"), "{head}");
        assert_eq!(body["reason"], "AGENT_DECISION_LIMIT");
        assert_eq!(body["retry_after_secs"], 754);
        assert_eq!(body["limit"], 20);

        let (head, body) = answer(ReviewRepositoryError::HumanRequired {
            review_id: CrawlerReviewId::new(),
            agent_rounds: 3,
            max_repair_rounds: 3,
        });
        assert!(head.starts_with("HTTP/1.1 409 Conflict"), "{head}");
        assert_eq!(body["reason"], "HUMAN_REQUIRED");
        assert_eq!(body["agent_rounds"], 3);
        assert_eq!(body["limit"], 3);
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|error| error.contains("waits for a human")),
            "{body}"
        );
    }

    /// An approval names the candidate version its approver read, like a candidate edit.
    #[test]
    fn should_refuse_an_approval_that_names_no_candidate_version() {
        for body in ["", r#"{"notes": "looks right"}"#] {
            let refused = parse_verdict_payload::<ApprovalPayload>(body).err();
            assert!(
                refused
                    .is_some_and(|error| error.to_string().contains("expected_candidate_version")),
                "{body:?} must be refused"
            );
        }
        let payload = parse_verdict_payload::<ApprovalPayload>(
            r#"{"expected_candidate_version": 4, "notes": "looks right"}"#,
        )
        .unwrap_or_else(|error| panic!("versioned approval: {error}"));
        let approval = payload.approval();
        assert_eq!(approval.expected_candidate_version, 4);
        assert_eq!(approval.notes, Some("looks right"));
        assert_eq!(approval.incomplete_coverage, IncompleteCoverage::Refuse);
        assert!(
            parse_verdict_payload::<ActionPayload>("").is_ok_and(|payload| payload.notes.is_none())
        );
    }

    mod matrix_refresh {
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, send_as};
        use super::{AgentDecisionPolicy, ReviewServer, ReviewServerConfig, agent_mode};
        use crate::CrawlerReviewId;
        use crate::network::policy::NetworkErrorKind;
        use crate::review::agent_decisions::AgentLimits;
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::matrix_refresh::MatrixRefreshStatus;
        use crate::review::model::{PAGE_ROLE_PRIMARY, ReviewActor, SchemaReviewPageInput};
        use crate::review::repository::CrawlerReviewRepository;
        use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
        use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
        use crate::scraper::css_selector::rule::{
            ExtractionCardinality, ExtractionKind, ExtractionRule,
        };
        use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use sqlx::PgPool;
        use std::collections::HashMap;
        use std::net::SocketAddr;
        use std::num::NonZeroU32;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};
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
            html: String,
        }

        impl PacedFetcher {
            fn new(failure: Option<&'static str>) -> Arc<Self> {
                Arc::new(Self {
                    released: Semaphore::new(0),
                    requested: AtomicUsize::new(0),
                    fetched: AtomicUsize::new(0),
                    failure,
                    html: PAGE_HTML.to_owned(),
                })
            }

            /// A shop that serves `html` for every page.
            fn serving(html: String) -> Arc<Self> {
                Arc::new(Self {
                    released: Semaphore::new(0),
                    requested: AtomicUsize::new(0),
                    fetched: AtomicUsize::new(0),
                    failure: None,
                    html,
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
                    html: self.html.clone(),
                    final_url: url.clone(),
                    status: reqwest::StatusCode::OK,
                })
            }
        }

        /// The product page nested in `depth` `div`s, which html5ever parses ever more slowly as
        /// the nesting deepens.
        fn deeply_nested_page(depth: usize) -> String {
            PAGE_HTML
                .replace("<body>", &format!("<body>{}", "<div>".repeat(depth)))
                .replace("</body>", &format!("{}</body>", "</div>".repeat(depth)))
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

        /// The console of the paced shop: tokenless, without agents, with the default limit.
        fn paced_console() -> ReviewServerConfig {
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
                agent_tokens: None,
                max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
                local_mutations_without_token: false,
            }
        }

        async fn fixture(fetcher: Arc<PacedFetcher>) -> Fixture {
            fixture_with(fetcher, paced_console(), AgentDecisionPolicy::default()).await
        }

        /// A paced review over [`PAGE_COUNT`] pages on a `console` whose repository applies
        /// `agent_decisions`.
        async fn fixture_with(
            fetcher: Arc<PacedFetcher>,
            console: ReviewServerConfig,
            agent_decisions: AgentDecisionPolicy,
        ) -> Fixture {
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
            let repository =
                CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(agent_decisions);
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
                ..ReviewServer::new(repository, console, fetcher.clone())
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
                        server
                            .matrix_refreshes
                            .status(review_id, ReviewActor::Human),
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
                &[],
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
                    ReviewActor::Human,
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
            let joined_page = repository
                .get_review_pages(fixture.review_id)
                .await
                .unwrap_or_else(|error| panic!("read review pages: {error}"))
                .into_iter()
                .find(|page| page.url == joining_url.as_str())
                .unwrap_or_else(|| panic!("the joining page is a review page"));
            let answered: serde_json::Value = settled
                .split_once("\r\n\r\n")
                .and_then(|(_, body)| serde_json::from_str(body).ok())
                .unwrap_or_else(|| panic!("matrix answer is not JSON: {settled}"));
            assert_eq!(
                answered["not_evaluated_pages"],
                json!([joined_page.review_page_id]),
                "the answer names the page the matrix did not evaluate"
            );
        }

        /// A plain read never starts a fetch: without a stored matrix it says so, and only an
        /// explicit refresh fetches the review's pages.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_no_matrix_without_fetching_when_the_review_stores_none() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture(fetcher.clone()).await;

            let read = get_matrix(&fixture.server, fixture.review_id, false).await;
            tokio::task::yield_now().await;

            assert!(read.starts_with("HTTP/1.1 404"), "{read}");
            assert!(read.contains("\"reason\": \"NO_MATRIX\""), "{read}");
            assert_eq!(fetcher.requested.load(Ordering::SeqCst), 0);
            assert!(matches!(
                fixture
                    .server
                    .matrix_refreshes
                    .status(fixture.review_id, ReviewActor::Human),
                MatrixRefreshStatus::Idle
            ));
        }

        /// Refreshes, candidate evaluations and page projections share the console's limit: a
        /// request that would start one above it is refused before it fetches anything, joining a
        /// running refresh takes no place, and a finished refresh frees its place.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_live_jobs_above_the_console_limit_with_retry_after() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture_with(
                fetcher.clone(),
                ReviewServerConfig {
                    max_concurrent_live_jobs: NonZeroU32::MIN,
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    ..paced_console()
                },
                agent_mode(AgentLimits::default()),
            )
            .await;
            let review_id = fixture.review_id;
            let page_id = CrawlerReviewRepository::new(fixture.pool.clone())
                .get_review(review_id)
                .await
                .unwrap_or_else(|error| panic!("read review: {error}"))
                .pages[0]
                .review_page_id;
            let project = format!(
                "GET /api/reviews/{review_id}/page-projection?page={page_id} HTTP/1.1\r\nHost: localhost\r\n\r\n"
            );
            // The author agent's evaluation, a POST its token may send to the tokenless console.
            let evaluate = format!(
                "POST /api/reviews/{review_id}/evaluate HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {AUTHOR_TOKEN}\r\n\r\n{}",
                json!({ "schemas": [schema("h1")] })
            );

            let refreshing = get_matrix(&fixture.server, review_id, true).await;
            let joined = get_matrix(&fixture.server, review_id, true).await;
            let evaluation = fixture.server.route(&evaluate).await.to_string();
            let projection = fixture.server.route(&project).await.to_string();

            assert!(refreshing.starts_with("HTTP/1.1 202"), "{refreshing}");
            assert!(joined.starts_with("HTTP/1.1 202"), "{joined}");
            for refused in [&evaluation, &projection] {
                assert!(refused.starts_with("HTTP/1.1 429"), "{refused}");
                assert!(refused.contains("Retry-After: 30\r\n"), "{refused}");
                assert!(
                    refused.contains("\"reason\": \"LIVE_JOBS_BUSY\""),
                    "{refused}"
                );
                assert!(refused.contains("\"limit\": 1"), "{refused}");
            }
            fetcher.release(PAGE_COUNT);
            let settled = poll_until_settled(&fixture.server, review_id).await;
            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            assert_eq!(fetcher.requested.load(Ordering::SeqCst), PAGE_COUNT);
            fetcher.release(1);
            let projected = fixture.server.route(&project).await.to_string();
            assert!(projected.starts_with("HTTP/1.1 200"), "{projected}");
        }

        /// A paced review on a console in `agent` mode whose agents reach the repair cap with one
        /// needs-repair round.
        async fn one_round_fixture(fetcher: Arc<PacedFetcher>) -> Fixture {
            fixture_with(
                fetcher,
                ReviewServerConfig {
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    ..paced_console()
                },
                agent_mode(AgentLimits {
                    max_repair_rounds: NonZeroU32::MIN,
                    ..AgentLimits::default()
                }),
            )
            .await
        }

        /// Starts the reviewer agent's refresh of the fixture's review, has the author agent's
        /// needs-repair verdict bring the review to the cap while the pages are fetched, and
        /// answers the reviewer's first plain matrix read after the refresh settled.
        async fn refresh_refused_for_a_human(
            fixture: &Fixture,
            fetcher: &PacedFetcher,
        ) -> (String, serde_json::Value) {
            let review_id = fixture.review_id;
            let matrix = format!("GET /api/reviews/{review_id}/matrix");
            let (status, started) = send_as(
                &fixture.server,
                REVIEWER_TOKEN,
                &format!("{matrix}?refresh=true"),
                "",
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 202"), "{status} {started}");
            fetcher.wait_for_first_request().await;
            let (status, verdict) = send_as(
                &fixture.server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/needs-repair"),
                &json!({ "notes": "the title takes the shop name" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {verdict}");
            fetcher.release(PAGE_COUNT);
            for _ in 0..200 {
                let (status, answer) = send_as(&fixture.server, REVIEWER_TOKEN, &matrix, "").await;
                if !status.starts_with("HTTP/1.1 202") {
                    return (status, answer);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("the agent's refresh did not settle");
        }

        /// A review agent's refresh that the review refuses because it came to wait for a human
        /// while the pages were fetched answers that agent only: a human, or the other agent,
        /// reading the matrix next learns that the review holds none, without the words written
        /// for the agents.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_an_agent_refusal_only_to_the_agent_whose_refresh_it_was() {
            let fetcher = PacedFetcher::new(None);
            let fixture = one_round_fixture(fetcher.clone()).await;
            let review_id = fixture.review_id;
            let matrix = format!("GET /api/reviews/{review_id}/matrix");
            let agent_text = "leave it until a human changes or judges it";

            let (status, refused) = refresh_refused_for_a_human(&fixture, &fetcher).await;
            let human = get_matrix(&fixture.server, review_id, false).await;
            let (author_status, author) = send_as(&fixture.server, AUTHOR_TOKEN, &matrix, "").await;

            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");
            assert_eq!(refused["reason"], "HUMAN_REQUIRED", "{refused}");
            assert!(human.starts_with("HTTP/1.1 404"), "{human}");
            assert!(human.contains("\"reason\": \"NO_MATRIX\""), "{human}");
            assert!(!human.contains(agent_text), "{human}");
            assert!(
                author_status.starts_with("HTTP/1.1 404"),
                "{author_status} {author}"
            );
            assert_eq!(author["reason"], "NO_MATRIX", "{author}");
            assert!(stored_matrix(&fixture.pool, review_id).await.is_none());
        }

        /// Once a human hands the review back, the agent whose refresh the review refused reads
        /// the matrix as everyone else does: its refusal from before the hand-back is stale.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_no_refusal_from_before_the_hand_back_to_the_agent() {
            let fetcher = PacedFetcher::new(None);
            let fixture = one_round_fixture(fetcher.clone()).await;
            let review_id = fixture.review_id;
            let (status, refused) = refresh_refused_for_a_human(&fixture, &fetcher).await;
            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");

            CrawlerReviewRepository::new(fixture.pool.clone())
                .request_repair(
                    review_id,
                    ReviewActor::Human,
                    Some("keep the title on the h1 and try again"),
                )
                .await
                .unwrap_or_else(|error| panic!("hand the review back: {error}"));
            let (status, answer) = send_as(
                &fixture.server,
                REVIEWER_TOKEN,
                &format!("GET /api/reviews/{review_id}/matrix"),
                "",
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 404"), "{status} {answer}");
            assert_eq!(answer["reason"], "NO_MATRIX", "{answer}");
        }

        /// A refresh and a candidate evaluation each evaluate their pages on a blocking thread
        /// within the place their live job holds, so with a limit of one neither refuses its own
        /// evaluation, and each frees its place once it answered.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_evaluate_within_the_live_jobs_own_place_when_the_limit_is_one() {
            let fetcher = PacedFetcher::new(None);
            let fixture = fixture_with(
                fetcher.clone(),
                ReviewServerConfig {
                    max_concurrent_live_jobs: NonZeroU32::MIN,
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    ..paced_console()
                },
                agent_mode(AgentLimits::default()),
            )
            .await;
            let server = ReviewServer {
                matrix_refresh_wait: Duration::from_secs(30),
                ..fixture.server.clone()
            };
            let review_id = fixture.review_id;
            fetcher.release(2 * PAGE_COUNT);

            let refreshed = get_matrix(&server, review_id, true).await;
            let refresh_freed_its_place = server.live_jobs.try_start().is_ok();
            let (status, evaluated) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/evaluate"),
                &json!({ "schemas": [schema("h1")] }).to_string(),
            )
            .await;
            let evaluation_freed_its_place = server.live_jobs.try_start().is_ok();

            assert!(refreshed.starts_with("HTTP/1.1 200"), "{refreshed}");
            assert!(stored_matrix(&fixture.pool, review_id).await.is_some());
            assert!(refresh_freed_its_place);
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {evaluated}");
            assert_eq!(
                evaluated["candidates"][0]["pages"][0]["apply_ok"], true,
                "{evaluated}"
            );
            assert!(evaluation_freed_its_place);
            assert_eq!(fetcher.fetched.load(Ordering::SeqCst), 2 * PAGE_COUNT);
        }

        /// Parsing a deeply nested page holds up whatever thread parses it. A refresh parses on a
        /// blocking thread, so the console keeps answering other requests while it evaluates: no
        /// request waits for a sizeable part of the refresh, as each would on the one async worker
        /// of this test's runtime if the refresh parsed there.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_other_requests_while_a_refresh_evaluates_a_deeply_nested_page() {
            let fetcher = PacedFetcher::serving(deeply_nested_page(4_000));
            let fixture = fixture(fetcher.clone()).await;
            fetcher.release(PAGE_COUNT);
            let reviews = "GET /api/reviews HTTP/1.1\r\nHost: localhost\r\n\r\n";

            let refresh_started = Instant::now();
            let refreshing = get_matrix(&fixture.server, fixture.review_id, true).await;
            // The refresh request counts as one of the waits: with the sleeps between them, the
            // waits cover the whole refresh, so a parse on the async worker lands in one of them,
            // usually the refresh request's own short wait for the outcome.
            let mut longest_wait = refresh_started.elapsed();
            while matches!(
                fixture
                    .server
                    .matrix_refreshes
                    .status(fixture.review_id, ReviewActor::Human),
                MatrixRefreshStatus::Running
            ) {
                let asked = Instant::now();
                let listed = fixture.server.route(reviews).await.to_string();
                tokio::time::sleep(Duration::from_millis(10)).await;
                longest_wait = longest_wait.max(asked.elapsed());
                assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
            }
            let refresh_took = refresh_started.elapsed();
            let settled = get_matrix(&fixture.server, fixture.review_id, false).await;

            assert!(
                refreshing.starts_with("HTTP/1.1 202") || refreshing.starts_with("HTTP/1.1 200"),
                "{refreshing}"
            );
            assert!(settled.starts_with("HTTP/1.1 200"), "{settled}");
            assert!(
                longest_wait < refresh_took / 4,
                "a request waited {longest_wait:?} of the refresh's {refresh_took:?}"
            );
        }
    }

    mod needs_repair {
        use super::matrix_refresh::schema;
        use super::{ReviewServer, ReviewServerConfig};
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        pub(super) const TOKEN: &str = "review-token";
        pub(super) const AUTHOR_TOKEN: &str = "author-token";
        pub(super) const REVIEWER_TOKEN: &str = "reviewer-token";

        /// Sends an authorized request and returns its status line and JSON body.
        pub(super) async fn send(
            server: &ReviewServer,
            request: &str,
            body: &str,
        ) -> (String, serde_json::Value) {
            send_as(server, TOKEN, request, body).await
        }

        /// Sends a request with `token` as its bearer and returns its status line and JSON body.
        pub(super) async fn send_as(
            server: &ReviewServer,
            token: &str,
            request: &str,
            body: &str,
        ) -> (String, serde_json::Value) {
            let response = server
                .route(&format!(
                    "{request} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n{body}"
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
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: None,
                    local_mutations_without_token: false,
                    max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send};
        use super::{AgentDecisionPolicy, ReviewServer, ReviewServerConfig, agent_mode};
        use crate::CrawlerReviewId;
        use crate::review::agent_decisions::AgentLimits;
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        /// A review server of `agent` authoring mode over a pending manual schema review, and that
        /// review.
        pub(super) async fn pending_review_server() -> (ReviewServer, CrawlerReviewId) {
            pending_review_server_deciding(agent_mode(AgentLimits::default())).await
        }

        /// As [`pending_review_server`], taking the agents' decisions by `agent_decisions`.
        pub(super) async fn pending_review_server_deciding(
            agent_decisions: AgentDecisionPolicy,
        ) -> (ReviewServer, CrawlerReviewId) {
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
            let repository =
                CrawlerReviewRepository::new(pool).with_agent_decisions(agent_decisions);
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
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    local_mutations_without_token: false,
                    max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
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

        /// A refused schema edit answers 400 with the schema, the field, the selector and why
        /// the selector is invalid, which the console shows next to the edit.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_name_the_field_selector_and_reason_of_a_refused_schema_edit() {
            let (server, review_id) = pending_review_server().await;
            let version = candidate_version(&server, review_id).await;

            let (field_status, field_refusal) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/schema-field"),
                &json!({
                    "expected_candidate_version": version,
                    "schema_index": 0,
                    "field": "price",
                    "rule": {"selector": "span[", "type": "text"},
                })
                .to_string(),
            )
            .await;
            let (candidate_status, candidate_refusal) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/candidate"),
                &json!({
                    "expected_candidate_version": version,
                    "schemas": [schema("h1"), schema("h1[")],
                })
                .to_string(),
            )
            .await;

            for (status, refusal, named) in [
                (
                    field_status,
                    field_refusal,
                    "schema 0, field `price`: invalid CSS selector `span[`: ",
                ),
                (
                    candidate_status,
                    candidate_refusal,
                    "schema 1, field `title`: invalid CSS selector `h1[`: ",
                ),
            ] {
                assert!(status.starts_with("HTTP/1.1 400"), "{status} {refusal}");
                assert!(
                    refusal["error"].as_str().is_some_and(|error| error
                        .starts_with(&format!("invalid ProductSchema review candidate: {named}"))
                        && error.len()
                            > "invalid ProductSchema review candidate: ".len() + named.len()),
                    "{refusal}"
                );
            }
        }

        /// The answer to a matrix refresh of `review_id` once it is no longer running.
        async fn refused_matrix_refresh(
            server: &ReviewServer,
            review_id: CrawlerReviewId,
        ) -> (String, serde_json::Value) {
            let mut answer = send(
                server,
                &format!("GET /api/reviews/{review_id}/matrix?refresh=true"),
                "",
            )
            .await;
            for _ in 0..100 {
                if !answer.0.starts_with("HTTP/1.1 202") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                answer = send(server, &format!("GET /api/reviews/{review_id}/matrix"), "").await;
            }
            answer
        }

        /// Counts ERROR events.
        #[derive(Clone, Default)]
        struct ErrorEvents(Arc<std::sync::atomic::AtomicUsize>);

        impl ErrorEvents {
            fn count(&self) -> usize {
                self.0.load(std::sync::atomic::Ordering::SeqCst)
            }
        }

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ErrorEvents {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                if *event.metadata().level() == tracing::Level::ERROR {
                    self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }

        /// A pending review whose persisted candidate fails validation is logged at ERROR once
        /// per load of its detail, which shows the failure as `candidate_error`; the matrix read
        /// and the repository's other reads of the same review log nothing.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_log_an_invalid_persisted_candidate_once_per_detail_load() {
            use tracing::instrument::WithSubscriber as _;
            use tracing_subscriber::layer::SubscriberExt as _;

            let (server, review_id) = pending_review_server().await;
            let mut invalid = schema("h1");
            invalid.title.selector = "h1[".into();
            sqlx::query("UPDATE crawler_reviews SET candidate_payload = $2 WHERE review_id = $1")
                .bind(review_id.as_uuid())
                .bind(json!({ "schemas": [invalid] }))
                .execute(&test_api::get_postgres_client().await)
                .await
                .unwrap_or_else(|error| panic!("store an invalid candidate: {error}"));
            let errors = ErrorEvents::default();
            let review = format!("GET /api/reviews/{review_id}");
            let matrix = format!("GET /api/reviews/{review_id}/matrix");

            let (logged_by_reads, detail, matrix_status, logged_by_detail) = async {
                server
                    .repository
                    .get_review(review_id)
                    .await
                    .unwrap_or_else(|error| panic!("read review: {error}"));
                let (matrix_status, _) = send(&server, &matrix, "").await;
                let (second_matrix_status, _) = send(&server, &matrix, "").await;
                assert_eq!(matrix_status, second_matrix_status);
                let logged_by_reads = errors.count();
                let (_, detail) = send(&server, &review, "").await;
                (logged_by_reads, detail, matrix_status, errors.count())
            }
            .with_subscriber(tracing_subscriber::registry().with(errors.clone()))
            .await;

            let refused_refresh = refused_matrix_refresh(&server, review_id).await;

            assert_eq!(logged_by_reads, 0);
            assert!(matrix_status.starts_with("HTTP/1.1 404"), "{matrix_status}");
            assert!(
                refused_refresh.0.starts_with("HTTP/1.1 400")
                    && refused_refresh.1["error"].as_str().is_some_and(|error| error.starts_with(
                        "invalid ProductSchema review candidate: schema 0, field `title`: invalid CSS selector `h1[`: "
                    )),
                "{refused_refresh:?}"
            );
            assert_eq!(logged_by_detail, 1);
            assert!(
                detail["candidate_error"].as_str().is_some_and(|error| error
                    .starts_with("schema 0, field `title`: invalid CSS selector `h1[`: ")),
                "{detail}"
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

    mod review_event_trail {
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send, send_as};
        use super::schema_editing::pending_review_server;
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        /// The console records each edit as made by the actor of the credential it came with,
        /// records nothing for a refused edit, and returns the trail with the review. An agent
        /// token opens no browser session.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_record_the_actor_of_each_credential_and_return_the_trail_with_the_review() {
            let (server, review_id) = pending_review_server().await;
            let schema_field = format!("POST /api/reviews/{review_id}/schema-field");
            let review = format!("GET /api/reviews/{review_id}");
            let (_, before) = send(&server, &review, "").await;
            let version = before["review"]["candidate_version"]
                .as_i64()
                .unwrap_or_else(|| panic!("review without candidate version: {before}"));
            let title_edit = |version: i64, selector: &str| {
                json!({
                    "expected_candidate_version": version,
                    "schema_index": 0,
                    "field": "title",
                    "rule": {"selector": selector, "type": "text"},
                })
                .to_string()
            };

            let (agent, agent_answer) = send_as(
                &server,
                AUTHOR_TOKEN,
                &schema_field,
                &title_edit(version, "h1.agent"),
            )
            .await;
            let (human, human_answer) = send_as(
                &server,
                TOKEN,
                &schema_field,
                &title_edit(version + 1, "h1.human"),
            )
            .await;
            let (stale, stale_answer) = send_as(
                &server,
                AUTHOR_TOKEN,
                &schema_field,
                &title_edit(version, "h1.stale"),
            )
            .await;
            let session = server
                .route(&format!(
                    "POST /api/session HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {REVIEWER_TOKEN}\r\n\r\n"
                ))
                .await
                .to_string();

            assert!(agent.starts_with("HTTP/1.1 200"), "{agent} {agent_answer}");
            assert!(human.starts_with("HTTP/1.1 200"), "{human} {human_answer}");
            assert!(stale.starts_with("HTTP/1.1 409"), "{stale} {stale_answer}");
            assert!(session.starts_with("HTTP/1.1 401"), "{session}");
            assert!(!session.contains("Set-Cookie"), "{session}");
            let (status, detail) = send_as(&server, REVIEWER_TOKEN, &review, "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {detail}");
            let events = detail["events"]
                .as_array()
                .unwrap_or_else(|| panic!("review without events: {detail}"));
            let trail: Vec<_> = events
                .iter()
                .map(|event| {
                    (
                        event["actor"].clone(),
                        event["action"].clone(),
                        event["candidate_version"].clone(),
                    )
                })
                .collect();
            assert_eq!(
                trail,
                vec![
                    (
                        json!("AGENT_AUTHOR"),
                        json!("SCHEMA_FIELD_EDITED"),
                        json!(version + 1)
                    ),
                    (
                        json!("HUMAN"),
                        json!("SCHEMA_FIELD_EDITED"),
                        json!(version + 2)
                    ),
                ]
            );
            assert!(
                events.iter().all(|event| event["event_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("cre_"))),
                "{detail}"
            );
        }

        /// The events of all reviews since a time come back oldest first with each review's artifact
        /// type and ListingSource and without their details, to any credential that reads the
        /// console, such as the author agent's for the runner's digest; a time that is no RFC 3339
        /// time is refused.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_list_the_events_of_all_reviews_since_a_time() {
            let (server, review_id) = pending_review_server().await;
            let schema_field = format!("POST /api/reviews/{review_id}/schema-field");
            let edit = |version: i64| {
                json!({
                    "expected_candidate_version": version,
                    "schema_index": 0,
                    "field": "title",
                    "rule": {"selector": "h1", "type": "text"},
                })
                .to_string()
            };
            let before = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
            let (_, review) = send(&server, &format!("GET /api/reviews/{review_id}"), "").await;
            let version = review["review"]["candidate_version"]
                .as_i64()
                .unwrap_or_default();
            let (edited, _) = send_as(&server, AUTHOR_TOKEN, &schema_field, &edit(version)).await;
            let (repaired, _) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/needs-repair"),
                &json!({ "notes": "price empty on /item/2" }).to_string(),
            )
            .await;
            let since = |at: time::OffsetDateTime| {
                url::form_urlencoded::byte_serialize(
                    at.format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default()
                        .as_bytes(),
                )
                .collect::<String>()
            };

            let (status, listed) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("GET /api/review-events?since={}", since(before)),
                "",
            )
            .await;
            let (_, later) = send(
                &server,
                &format!(
                    "GET /api/review-events?since={}",
                    since(time::OffsetDateTime::now_utc() + time::Duration::minutes(1))
                ),
                "",
            )
            .await;
            let (refused, _) = send(&server, "GET /api/review-events?since=yesterday", "").await;

            assert!(edited.starts_with("HTTP/1.1 200"), "{edited}");
            assert!(repaired.starts_with("HTTP/1.1 200"), "{repaired}");
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {listed}");
            assert_eq!(listed["truncated"], json!(false));
            let events: Vec<_> = listed["events"]
                .as_array()
                .unwrap_or_else(|| panic!("no events: {listed}"))
                .iter()
                .map(|event| {
                    (
                        event["review_id"].clone(),
                        event["artifact_type"].clone(),
                        event["actor"].clone(),
                        event["action"].clone(),
                        event["notes"].clone(),
                        event.get("details").is_none(),
                    )
                })
                .collect();
            assert_eq!(
                events,
                vec![
                    (
                        json!(review_id.to_string()),
                        json!("PRODUCT_SCHEMA"),
                        json!("AGENT_AUTHOR"),
                        json!("SCHEMA_FIELD_EDITED"),
                        json!(null),
                        true
                    ),
                    (
                        json!(review_id.to_string()),
                        json!("PRODUCT_SCHEMA"),
                        json!("HUMAN"),
                        json!("REPAIR_REQUESTED"),
                        json!("price empty on /item/2"),
                        true
                    ),
                ]
            );
            assert_eq!(later["events"], json!([]));
            assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
        }

        /// The author agent changes no decided review: its edits and page additions to the
        /// approved review whose candidate is live answer 403 with an `error`, before any page is
        /// fetched, and record nothing, while the review token's edit still applies.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_the_agent_changes_of_an_approved_review_with_403() {
            let (server, review_id) = pending_review_server().await;
            let review = format!("GET /api/reviews/{review_id}");
            let approve = format!("POST /api/reviews/{review_id}/approve");
            let (_, incomplete) = send(
                &server,
                &approve,
                &json!({ "expected_candidate_version": 1 }).to_string(),
            )
            .await;
            let (status, approved) = send(
                &server,
                &approve,
                &json!({
                    "expected_candidate_version": 1,
                    "acknowledged_coverage": incomplete["coverage"],
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {approved}");
            let (_, detail) = send(&server, &review, "").await;
            let version = detail["review"]["candidate_version"]
                .as_i64()
                .unwrap_or_else(|| panic!("review without candidate version: {detail}"));
            let title_edit = json!({
                "expected_candidate_version": version,
                "schema_index": 0,
                "field": "title",
                "rule": {"selector": "h1.edited", "type": "text"},
            })
            .to_string();

            for (request, body) in [
                (
                    format!("POST /api/reviews/{review_id}/schema-field"),
                    title_edit.clone(),
                ),
                (
                    format!("POST /api/reviews/{review_id}/candidate"),
                    json!({
                        "expected_candidate_version": version,
                        "schemas": [super::matrix_refresh::schema("h1.agent")],
                    })
                    .to_string(),
                ),
                (
                    format!("POST /api/reviews/{review_id}/pages"),
                    json!({ "url": "https://edited.example/products/1" }).to_string(),
                ),
            ] {
                let (status, refused) = send_as(&server, AUTHOR_TOKEN, &request, &body).await;
                assert!(
                    status.starts_with("HTTP/1.1 403 Forbidden"),
                    "{request}: {status} {refused}"
                );
                assert_eq!(
                    refused["reason"], "AGENT_CHANGES_ONLY_PENDING_REVIEWS",
                    "{request}: {refused}"
                );
            }
            let (status, edited) = send_as(
                &server,
                TOKEN,
                &format!("POST /api/reviews/{review_id}/schema-field"),
                &title_edit,
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {edited}");

            let (_, detail) = send(&server, &review, "").await;
            let trail: Vec<_> = detail["events"]
                .as_array()
                .unwrap_or_else(|| panic!("review without events: {detail}"))
                .iter()
                .map(|event| (event["actor"].clone(), event["action"].clone()))
                .collect();
            assert_eq!(
                trail,
                vec![
                    (json!("HUMAN"), json!("APPROVED")),
                    (json!("HUMAN"), json!("SCHEMA_FIELD_EDITED")),
                ]
            );
        }
    }

    /// The console takes the agents' verdicts by its own settings, and only a human restores.
    mod agent_decisions {
        use super::agent_mode;
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::schema_editing::{pending_review_server, pending_review_server_deciding};
        use crate::review::agent_decisions::AgentLimits;
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        /// The console of `agent` mode takes the reviewer agent's approval only within the approval
        /// guard.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_decide_the_reviewer_agent_s_approval_by_the_approval_guard() {
            let (guarded, guarded_review) = pending_review_server().await;
            let (status, refused) = send_as(
                &guarded,
                REVIEWER_TOKEN,
                &format!("POST /api/reviews/{guarded_review}/approve"),
                &json!({ "expected_candidate_version": 1 }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");
            assert_eq!(
                refused["failed_checks"],
                json!([{ "check": "LIVE_MATRIX_MISSING" }])
            );
            let (_, detail) = send_as(
                &guarded,
                TOKEN,
                &format!("GET /api/reviews/{guarded_review}"),
                "",
            )
            .await;
            assert_eq!(detail["review"]["status"], "PENDING_REVIEW");
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_restore_an_approval_for_a_human_only() {
            let (server, review_id) = pending_review_server().await;
            let approve = format!("POST /api/reviews/{review_id}/approve");
            let restore = format!("POST /api/reviews/{review_id}/restore");
            let (_, incomplete) = send_as(
                &server,
                TOKEN,
                &approve,
                &json!({ "expected_candidate_version": 1 }).to_string(),
            )
            .await;
            let (status, approved) = send_as(
                &server,
                TOKEN,
                &approve,
                &json!({
                    "expected_candidate_version": 1,
                    "acknowledged_coverage": incomplete["coverage"],
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {approved}");

            let (agent_status, by_agent) = send_as(&server, REVIEWER_TOKEN, &restore, "").await;
            let (status, restored) = send_as(
                &server,
                TOKEN,
                &restore,
                &json!({ "notes": "wrong schema" }).to_string(),
            )
            .await;
            let (again_status, again) = send_as(&server, TOKEN, &restore, "").await;

            assert!(
                agent_status.starts_with("HTTP/1.1 403"),
                "{agent_status} {by_agent}"
            );
            assert_eq!(by_agent["reason"], "RESTORE_NEEDS_HUMAN");
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {restored}");
            assert!(
                again_status.starts_with("HTTP/1.1 409"),
                "{again_status} {again}"
            );
            let (_, detail) =
                send_as(&server, TOKEN, &format!("GET /api/reviews/{review_id}"), "").await;
            let last = detail["events"]
                .as_array()
                .and_then(|events| events.last())
                .unwrap_or_else(|| panic!("review without events: {detail}"));
            assert_eq!(last["action"], "RESTORED");
            assert_eq!(last["actor"], "HUMAN");
            assert_eq!(last["notes"], "wrong schema");
        }

        /// The reviewer agent's rejection is undone by a human, once and recorded as theirs; the
        /// review stays rejected. Nothing else revokes: not an agent, and not on a review without
        /// a rejection.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_revoke_a_rejection_for_a_human_only() {
            let (server, review_id) = pending_review_server().await;
            let revoke = format!("POST /api/reviews/{review_id}/revoke-rejection");
            let (pending_status, pending) = send_as(&server, TOKEN, &revoke, "").await;
            let (status, rejected) = send_as(
                &server,
                REVIEWER_TOKEN,
                &format!("POST /api/reviews/{review_id}/reject"),
                &json!({ "notes": "not a product page" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {rejected}");

            let (agent_status, by_agent) = send_as(&server, REVIEWER_TOKEN, &revoke, "").await;
            let (status, revoked) = send_as(
                &server,
                TOKEN,
                &revoke,
                &json!({ "notes": "the agent rejected product pages" }).to_string(),
            )
            .await;
            let (again_status, again) = send_as(&server, TOKEN, &revoke, "").await;

            assert!(
                pending_status.starts_with("HTTP/1.1 400"),
                "{pending_status} {pending}"
            );
            assert!(
                agent_status.starts_with("HTTP/1.1 403"),
                "{agent_status} {by_agent}"
            );
            assert_eq!(by_agent["reason"], "REVOKE_NEEDS_HUMAN");
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {revoked}");
            assert!(
                again_status.starts_with("HTTP/1.1 409"),
                "{again_status} {again}"
            );
            let (_, detail) =
                send_as(&server, TOKEN, &format!("GET /api/reviews/{review_id}"), "").await;
            assert_eq!(detail["review"]["status"], "REJECTED");
            let trail: Vec<(&str, &str)> = detail["events"]
                .as_array()
                .unwrap_or_else(|| panic!("review without events: {detail}"))
                .iter()
                .map(|event| {
                    (
                        event["actor"].as_str().unwrap_or_default(),
                        event["action"].as_str().unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                trail,
                vec![
                    ("AGENT_REVIEWER", "REJECTED"),
                    ("HUMAN", "REJECTION_REVOKED")
                ]
            );
            assert_eq!(
                detail["events"][1]["notes"],
                "the agent rejected product pages"
            );
        }

        /// Once the agents' needs-repair rounds reached the cap, the review list and the review
        /// say the review waits for a human, every agent route on it, the live jobs that fetch
        /// shop pages without changing it among them, answers 409 `HUMAN_REQUIRED` before any page
        /// is fetched and records nothing, the live previews refuse the agents' tokens as on any
        /// review, and a human's verdict hands it back.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_human_required_on_every_agent_route_once_the_repair_cap_is_reached()
        {
            let (server, review_id) = pending_review_server_deciding(agent_mode(AgentLimits {
                max_repair_rounds: std::num::NonZeroU32::MIN,
                ..AgentLimits::default()
            }))
            .await;
            let review = format!("GET /api/reviews/{review_id}");
            let needs_repair = format!("POST /api/reviews/{review_id}/needs-repair");
            let notes = json!({ "notes": "the title takes the shop name" }).to_string();
            let (status, taken) = send_as(&server, REVIEWER_TOKEN, &needs_repair, &notes).await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {taken}");
            let (_, list) = send_as(&server, TOKEN, "GET /api/reviews", "").await;
            let listed = list
                .as_array()
                .and_then(|reviews| {
                    reviews
                        .iter()
                        .find(|listed| listed["review_id"] == review_id.to_string())
                })
                .unwrap_or_else(|| panic!("review not listed: {list}"));
            assert_eq!(
                (&listed["agent_rounds"], &listed["human_required"]),
                (&json!(1), &json!(true))
            );
            let (_, detail) = send_as(&server, TOKEN, &review, "").await;
            assert_eq!(detail["review"]["human_required"], true);
            let version = detail["review"]["candidate_version"].clone();

            for (token, request, body) in [
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/candidate"),
                    json!({
                        "expected_candidate_version": version,
                        "schemas": [super::matrix_refresh::schema("h1.agent")],
                    }),
                ),
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/schema-field"),
                    json!({
                        "expected_candidate_version": version,
                        "schema_index": 0,
                        "field": "title",
                        "rule": {"selector": "h1.agent", "type": "text"},
                    }),
                ),
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/pages"),
                    json!({ "url": "https://edited.example/products/1" }),
                ),
                (
                    AUTHOR_TOKEN,
                    needs_repair.clone(),
                    json!({ "notes": "again" }),
                ),
                (
                    REVIEWER_TOKEN,
                    needs_repair.clone(),
                    json!({ "notes": "again" }),
                ),
                (
                    REVIEWER_TOKEN,
                    format!("GET /api/reviews/{review_id}/matrix?refresh=true"),
                    json!(null),
                ),
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/evaluate"),
                    json!({ "schemas": [super::matrix_refresh::schema("h1.agent")] }),
                ),
                (
                    AUTHOR_TOKEN,
                    format!(
                        "GET /api/reviews/{review_id}/page-projection?url=https%3A%2F%2Fedited.example%2Fproducts%2F1"
                    ),
                    json!(null),
                ),
                (
                    REVIEWER_TOKEN,
                    format!(
                        "GET /api/reviews/{review_id}/page-projection?url=https%3A%2F%2Fedited.example%2Fproducts%2F2"
                    ),
                    json!(null),
                ),
                (
                    REVIEWER_TOKEN,
                    format!("POST /api/reviews/{review_id}/approve"),
                    json!({ "expected_candidate_version": version }),
                ),
                (
                    REVIEWER_TOKEN,
                    format!("POST /api/reviews/{review_id}/reject"),
                    json!({ "notes": "not a product page" }),
                ),
            ] {
                let body = if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                };
                let (status, refused) = send_as(&server, token, &request, &body).await;
                assert!(
                    status.starts_with("HTTP/1.1 409 Conflict"),
                    "{request}: {status} {refused}"
                );
                assert_eq!(refused["reason"], "HUMAN_REQUIRED", "{request}: {refused}");
                assert_eq!(refused["agent_rounds"], 1, "{request}: {refused}");
                assert_eq!(refused["limit"], 1, "{request}: {refused}");
            }
            for route in ["/api/live-inspect", "/api/live-html"] {
                let request = format!(
                    "GET {route}?url=https%3A%2F%2Fedited.example%2Fproducts%2F3&review={review_id}"
                );
                for token in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                    let (status, refused) = send_as(&server, token, &request, "").await;
                    assert!(
                        status.starts_with("HTTP/1.1 401"),
                        "{request}: {status} {refused}"
                    );
                }
            }
            let (_, unchanged) = send_as(&server, TOKEN, &review, "").await;
            assert_eq!(unchanged["events"].as_array().map(Vec::len), Some(1));
            assert_eq!(unchanged["review"]["candidate_version"], version);

            let (status, handed_back) = send_as(
                &server,
                TOKEN,
                &needs_repair,
                &json!({ "notes": "take the h1 inside the product block" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {handed_back}");
            let (_, detail) = send_as(&server, TOKEN, &review, "").await;
            assert_eq!(
                (
                    &detail["review"]["agent_rounds"],
                    &detail["review"]["human_required"]
                ),
                (&json!(0), &json!(false))
            );
            let (status, edited) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/schema-field"),
                &json!({
                    "expected_candidate_version": version,
                    "schema_index": 0,
                    "field": "title",
                    "rule": {"selector": "h1.product", "type": "text"},
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {edited}");
        }
    }

    /// What each agent role may do on the console: the author stages, the reviewer judges, and
    /// each refusal names its reason and records nothing.
    mod agent_roles {
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::schema_editing::pending_review_server;
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_each_role_what_the_other_role_does_with_its_reason() {
            let (server, review_id) = pending_review_server().await;
            let title_edit = json!({
                "expected_candidate_version": 1,
                "schema_index": 0,
                "field": "title",
                "rule": {"selector": "h1.edited", "type": "text"},
            })
            .to_string();
            let approval = json!({ "expected_candidate_version": 1 }).to_string();
            let rejection = json!({ "notes": "not a product page" }).to_string();

            // The review server's fetcher expects no call: a refused page addition fetches
            // nothing.
            for (token, request, body, reason) in [
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/approve"),
                    approval.clone(),
                    "AUTHOR_CANNOT_DECIDE",
                ),
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/reject"),
                    rejection.clone(),
                    "AUTHOR_CANNOT_DECIDE",
                ),
                (
                    REVIEWER_TOKEN,
                    format!("POST /api/reviews/{review_id}/schema-field"),
                    title_edit.clone(),
                    "REVIEWER_CANNOT_CHANGE_CANDIDATES",
                ),
                (
                    REVIEWER_TOKEN,
                    format!("POST /api/reviews/{review_id}/candidate"),
                    json!({
                        "expected_candidate_version": 1,
                        "schemas": [super::matrix_refresh::schema("h1.reviewer")],
                    })
                    .to_string(),
                    "REVIEWER_CANNOT_CHANGE_CANDIDATES",
                ),
                (
                    REVIEWER_TOKEN,
                    format!("POST /api/reviews/{review_id}/pages"),
                    json!({ "url": "https://edited.example/products/1" }).to_string(),
                    "REVIEWER_CANNOT_CHANGE_CANDIDATES",
                ),
                (
                    AUTHOR_TOKEN,
                    format!("POST /api/reviews/{review_id}/restore"),
                    String::new(),
                    "RESTORE_NEEDS_HUMAN",
                ),
            ] {
                let (status, refused) = send_as(&server, token, &request, &body).await;
                assert!(
                    status.starts_with("HTTP/1.1 403 Forbidden"),
                    "{request}: {status} {refused}"
                );
                assert_eq!(refused["reason"], reason, "{request}: {refused}");
            }
            let (status, edited) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/schema-field"),
                &title_edit,
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {edited}");
            let (status, repair) = send_as(
                &server,
                REVIEWER_TOKEN,
                &format!("POST /api/reviews/{review_id}/needs-repair"),
                &json!({ "notes": "the title misses sold pages" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {repair}");

            let (_, detail) =
                send_as(&server, TOKEN, &format!("GET /api/reviews/{review_id}"), "").await;
            let trail: Vec<_> = detail["events"]
                .as_array()
                .unwrap_or_else(|| panic!("review without events: {detail}"))
                .iter()
                .map(|event| (event["actor"].clone(), event["action"].clone()))
                .collect();
            assert_eq!(
                trail,
                vec![
                    (json!("AGENT_AUTHOR"), json!("SCHEMA_FIELD_EDITED")),
                    (json!("AGENT_REVIEWER"), json!("REPAIR_REQUESTED")),
                ]
            );
            assert_eq!(detail["review"]["status"], "PENDING_REVIEW");
        }
    }

    /// The sample labels of a URL-pattern review on the console: the author agent labels, the
    /// reviewer agent approves once the decision follows the labels and labels nothing itself, and
    /// a URL-pattern review has no live matrix to evaluate.
    mod sample_labels {
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::{ReviewServer, ReviewServerConfig, agent_mode};
        use crate::CrawlerReviewId;
        use crate::review::agent_decisions::AgentLimits;
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::model::UrlPatternDecision;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::{Value, json};
        use sqlx::PgPool;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        const SHOP: &str = "https://antique-shop.example";
        const PRODUCTS: [&str; 3] = ["/item/oak-chair", "/item/pine-table", "/item/brass-lamp"];
        const NOT_PRODUCTS: [&str; 3] = ["/shop/chairs", "/about-us", "/impressum"];

        fn labels(products: &[&str], not_products: &[&str]) -> Value {
            let label = |path: &&str, label: &str| json!({ "url": format!("{SHOP}{path}"), "label": label, "reason": "seen on the page" });
            Value::Array(
                products
                    .iter()
                    .map(|path| label(path, "PRODUCT"))
                    .chain(not_products.iter().map(|path| label(path, "NOT_PRODUCT")))
                    .collect(),
            )
        }

        /// A review server taking the agents' guarded decisions, and the ListingSource and the
        /// pending, undecided URL-pattern review of the shop crawled on [`SHOP`].
        async fn url_pattern_review_server(
            pool: &PgPool,
        ) -> (ReviewServer, ListingSourceId, CrawlerReviewId) {
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
                 VALUES ($1, 'Antique shop', 'antique-shop', TRUE, NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
            crate::test_support::store_crawl_root(pool, listing_source_id, SHOP).await;
            let repository = CrawlerReviewRepository::new(pool.clone())
                .with_agent_decisions(agent_mode(AgentLimits::default()));
            let review_id = repository
                .create_url_pattern_review(
                    &listing_source_id,
                    "manual_url_pattern_required",
                    UrlPatternDecision::Undecided,
                    &["/item/oak-chair", "/item/pine-table", "/shop/chairs"]
                        .map(|path| format!("{SHOP}{path}")),
                    None,
                )
                .await
                .unwrap_or_else(|error| panic!("create URL-pattern review: {error}"));
            // The fetcher expects no call: labels and approvals fetch nothing.
            let server = ReviewServer::new(
                repository,
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    local_mutations_without_token: false,
                    max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
                },
                Arc::new(MockHtmlFetcher::new()),
            );
            (server, listing_source_id, review_id)
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_take_the_reviewer_approval_once_the_pattern_follows_the_author_labels() {
            let pool = test_api::get_postgres_client().await;
            let (server, listing_source_id, review_id) = url_pattern_review_server(&pool).await;
            let sample_labels = format!("POST /api/reviews/{review_id}/sample-labels");
            let candidate = format!("POST /api/reviews/{review_id}/candidate");
            let approve = format!("POST /api/reviews/{review_id}/approve");

            let (status, by_reviewer) = send_as(
                &server,
                REVIEWER_TOKEN,
                &sample_labels,
                &json!({ "expected_candidate_version": 1, "labels": labels(&PRODUCTS, &NOT_PRODUCTS) })
                    .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 403"), "{status} {by_reviewer}");
            assert_eq!(by_reviewer["reason"], "REVIEWER_CANNOT_CHANGE_CANDIDATES");
            let (status, unknown_label) = send_as(
                &server,
                AUTHOR_TOKEN,
                &sample_labels,
                &json!({
                    "expected_candidate_version": 1,
                    "labels": [{ "url": format!("{SHOP}/item/oak-chair"), "label": "CATEGORY", "reason": "a guess" }],
                })
                .to_string(),
            )
            .await;
            assert!(
                status.starts_with("HTTP/1.1 400"),
                "{status} {unknown_label}"
            );
            let (status, too_few) = send_as(
                &server,
                AUTHOR_TOKEN,
                &sample_labels,
                &json!({ "expected_candidate_version": 1, "labels": labels(&PRODUCTS, &NOT_PRODUCTS[..2]) })
                    .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {too_few}");
            let (status, decided) = send_as(
                &server,
                AUTHOR_TOKEN,
                &candidate,
                &json!({
                    "expected_candidate_version": too_few["candidate_version"],
                    "decision": { "type": "PATTERN", "value": "/item/[^/]+$" },
                    "current_pattern": null,
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {decided}");
            let (status, refused) = send_as(
                &server,
                REVIEWER_TOKEN,
                &approve,
                &json!({ "expected_candidate_version": decided["candidate_version"] }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");
            assert_eq!(
                refused["failed_checks"],
                json!([{ "check": "TOO_FEW_LABELS", "product": 3, "not_product": 2, "minimum": 3 }])
            );
            let (status, labelled) = send_as(
                &server,
                AUTHOR_TOKEN,
                &sample_labels,
                &json!({
                    "expected_candidate_version": decided["candidate_version"],
                    "labels": labels(&PRODUCTS, &NOT_PRODUCTS),
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {labelled}");
            let (status, matrix) = send_as(
                &server,
                REVIEWER_TOKEN,
                &format!("GET /api/reviews/{review_id}/matrix?refresh=true"),
                "",
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 400"), "{status} {matrix}");

            let (status, approved) = send_as(
                &server,
                REVIEWER_TOKEN,
                &approve,
                &json!({ "expected_candidate_version": labelled["candidate_version"] }).to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 200"), "{status} {approved}");
            let decision: (Option<String>, String) = sqlx::query_as(
                "SELECT url_pattern, url_pattern_state FROM listing_sources \
                 WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read the ListingSource's decision: {error}"));
            assert_eq!(
                decision,
                (Some("/item/[^/]+$".to_owned()), "MATCHED".to_owned())
            );
            let (_, detail) =
                send_as(&server, TOKEN, &format!("GET /api/reviews/{review_id}"), "").await;
            let trail: Vec<(Value, Value)> = detail["events"]
                .as_array()
                .unwrap_or_else(|| panic!("review without events: {detail}"))
                .iter()
                .map(|event| (event["actor"].clone(), event["action"].clone()))
                .collect();
            assert_eq!(
                trail,
                [
                    ("AGENT_AUTHOR", "CANDIDATE_EDITED"),
                    ("AGENT_AUTHOR", "CANDIDATE_EDITED"),
                    ("AGENT_AUTHOR", "CANDIDATE_EDITED"),
                    ("AGENT_REVIEWER", "APPROVED"),
                ]
                .map(|(actor, action)| (json!(actor), json!(action)))
            );
            let relied_on = &detail["events"][3]["details"]["sample_labels"];
            assert_eq!(relied_on["labels"].as_array().map(Vec::len), Some(6));
            assert_eq!(relied_on["labels"][0]["label"], "PRODUCT");
        }

        /// A label of a URL off the ListingSource's crawl host, which would say nothing about the
        /// decision, is refused with 400 and changes nothing.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_a_label_off_the_crawl_host() {
            let pool = test_api::get_postgres_client().await;
            let (server, _, review_id) = url_pattern_review_server(&pool).await;
            let off_host = "https://antique-shop.de/item/oak-chair";
            let mut off_host_labels = labels(&PRODUCTS, &NOT_PRODUCTS);
            off_host_labels[0]["url"] = json!(off_host);

            let (status, refused) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/sample-labels"),
                &json!({ "expected_candidate_version": 1, "labels": off_host_labels }).to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 400"), "{status} {refused}");
            assert!(
                refused["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(off_host)),
                "{refused}"
            );
            let (_, detail) =
                send_as(&server, TOKEN, &format!("GET /api/reviews/{review_id}"), "").await;
            assert_eq!(detail["review"]["candidate_version"], 1, "{detail}");
            assert_eq!(
                detail["review"]["candidate_payload"].get("sample_labels"),
                None,
                "{detail}"
            );
            assert_eq!(detail["events"], json!([]), "{detail}");
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
                    json!({ "expected_candidate_version": 1 }).to_string(),
                ),
                (
                    format!("POST /api/reviews/{unknown}/restore"),
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
                server
                    .matrix_refreshes
                    .status(unknown, crate::review::model::ReviewActor::Human),
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
                &json!({ "expected_candidate_version": 1, "notes": "looks right" }).to_string(),
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
                &json!({
                    "expected_candidate_version": 1,
                    "notes": "looks right",
                    "acknowledged_coverage": refused["coverage"],
                })
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
                &json!({
                    "expected_candidate_version": 1,
                    "notes": "looks right",
                    "acknowledged_coverage": stale["coverage"],
                })
                .to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 200"), "{status} {approved}");
            let (_, decided) = send(&server, &review, "").await;
            assert_eq!(decided["review"]["status"], "APPROVED");
        }
    }

    /// The console's page fetches for a review: a review page is previewed live for a human only,
    /// a page is projected as the schema prompt sees it, and a proposed candidate is evaluated on
    /// the review's pages and on URLs of its ListingSource without being stored.
    mod review_source_pages {
        use super::matrix_refresh::schema;
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::{ReviewServer, ReviewServerConfig};
        use crate::CrawlerReviewId;
        use crate::network::policy::NetworkErrorKind;
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::model::{PAGE_ROLE_PRIMARY, SchemaReviewPageInput};
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
        use listing_source_core::ListingSourceId;
        use serde_json::{Value, json};
        use sqlx::PgPool;
        use std::net::SocketAddr;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;
        use test_api::IntegrationTestService as _;
        use tokio::sync::Semaphore;
        use url::Url;

        const SHOP: &str = "https://antiques.example";

        /// A product page with JSON-LD product markup, titled by its slug.
        fn product_page(slug: &str) -> String {
            format!(
                r#"<html><head><script type="application/ld+json">{{"@type":"Product","name":"{slug}"}}</script></head><body><span class="id">{slug}</span><h1>{slug}</h1><span class="state">In stock</span><img class="product" src="{slug}.jpg"></body></html>"#
            )
        }

        /// The shop: every page is a product page named after its last path segment. With a gate,
        /// a page is served only once the test releases it, like a request queued behind a
        /// robots.txt `Crawl-delay`.
        #[derive(Default)]
        struct Shop {
            fetched: Mutex<Vec<String>>,
            gate: Option<Semaphore>,
        }

        impl Shop {
            fn fetched(&self) -> Vec<String> {
                self.fetched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
            }
        }

        #[async_trait::async_trait]
        impl HtmlFetcher for Shop {
            async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
                if let Some(gate) = &self.gate {
                    gate.acquire()
                        .await
                        .map_err(|error| FetchError::Network {
                            kind: NetworkErrorKind::Connect,
                            details: error.to_string(),
                        })?
                        .forget();
                }
                self.fetched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(url.to_string());
                let slug = url
                    .path_segments()
                    .and_then(Iterator::last)
                    .unwrap_or("page");
                Ok(FetchedHtml {
                    html: product_page(slug),
                    final_url: url.clone(),
                    status: reqwest::StatusCode::OK,
                })
            }
        }

        struct Fixture {
            pool: PgPool,
            server: ReviewServer,
            shop: Arc<Shop>,
            review_id: CrawlerReviewId,
        }

        /// A console of `agent` mode over a pending product-schema review of the ListingSource on
        /// `antiques.example`, whose one page is `/item/chair`.
        async fn fixture(shop: Shop, response_wait: Duration) -> Fixture {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
                 VALUES ($1, 'Antiques', 'antiques', TRUE, NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
            crate::test_support::store_crawl_root(&pool, listing_source_id, SHOP).await;
            let repository = CrawlerReviewRepository::new(pool.clone());
            let review_id = repository
                .create_schema_review(
                    &listing_source_id,
                    "manual_schema_required",
                    &[schema("h2")],
                    vec![SchemaReviewPageInput {
                        url: format!("{SHOP}/item/chair"),
                        role: PAGE_ROLE_PRIMARY.to_owned(),
                        raw_html: product_page("chair"),
                    }],
                    json!({}),
                )
                .await
                .unwrap_or_else(|error| panic!("create schema review: {error}"));
            let shop = Arc::new(shop);
            let server = ReviewServer {
                matrix_refresh_wait: response_wait,
                ..ReviewServer::new(
                    repository,
                    ReviewServerConfig {
                        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                        auth_token: Some(TOKEN.to_owned()),
                        agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                        local_mutations_without_token: false,
                        max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
                    },
                    shop.clone(),
                )
            };
            Fixture {
                pool,
                server,
                shop,
                review_id,
            }
        }

        fn encoded(url: &str) -> String {
            url::form_urlencoded::byte_serialize(url.as_bytes()).collect()
        }

        /// A review page's live previews fetch it for a human, as the browser console always did,
        /// and answer the review agents' tokens `401` without fetching it.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_preview_a_review_page_live_for_a_human_and_for_no_agent() {
            let Fixture {
                pool,
                server,
                shop,
                review_id,
            } = fixture(Shop::default(), Duration::from_secs(5)).await;
            let page_id: uuid::Uuid = sqlx::query_scalar(
                "SELECT review_page_id FROM crawler_review_pages WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("review page: {error}"));
            let page_id = crate::CrawlerReviewPageId::try_from(page_id)
                .unwrap_or_else(|error| panic!("{error}"));

            for route in ["inspect", "html"] {
                let request = format!("GET /api/review-pages/{page_id}/{route}");
                for agent in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
                    let (status, refused) = send_as(&server, agent, &request, "").await;
                    assert!(
                        status.starts_with("HTTP/1.1 401"),
                        "{request}: {status} {refused}"
                    );
                }
                assert!(shop.fetched().is_empty(), "{request}: {:?}", shop.fetched());
                let previewed = server
                    .route(&format!(
                        "{request} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
                    ))
                    .await
                    .to_string();
                assert!(
                    previewed.starts_with("HTTP/1.1 200"),
                    "{request}: {previewed}"
                );
                assert!(
                    previewed.contains("<h1>chair</h1>"),
                    "{request}: {previewed}"
                );
                assert_eq!(shop.fetched(), [format!("{SHOP}/item/chair")]);
                shop.fetched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clear();
            }
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_project_a_review_page_or_a_source_url_and_report_its_product_markup() {
            let Fixture {
                pool,
                server,
                shop,
                review_id,
            } = fixture(Shop::default(), Duration::from_secs(5)).await;
            let page_id: uuid::Uuid = sqlx::query_scalar(
                "SELECT review_page_id FROM crawler_review_pages WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("review page: {error}"));
            let page_id = crate::CrawlerReviewPageId::try_from(page_id)
                .unwrap_or_else(|error| panic!("{error}"));
            let route = format!("GET /api/reviews/{review_id}/page-projection");

            let (status, page) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("{route}?page={page_id}"),
                "",
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {page}");
            assert_eq!(page["review_page_id"], json!(page_id));
            assert_eq!(page["url"], format!("{SHOP}/item/chair"));
            assert_eq!(page["describes_one_product"], true);
            let projection = page["projection"].as_str().unwrap_or_default();
            assert_eq!(
                projection,
                crate::scraper::css_selector::product_schema_service::html_to_schema_prompt_dsl(
                    &product_page("chair")
                )
            );

            let table = encoded(&format!("{SHOP}/item/table#gallery"));
            let (status, url) =
                send_as(&server, REVIEWER_TOKEN, &format!("{route}?url={table}"), "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {url}");
            assert_eq!(url["review_page_id"], Value::Null);
            assert_eq!(url["url"], format!("{SHOP}/item/table"));
            assert_eq!(url["final_url"], format!("{SHOP}/item/table"));

            let foreign = encoded("https://elsewhere.example/item/1");
            for (query, expected) in [
                (format!("url={foreign}"), "HTTP/1.1 400"),
                (String::new(), "HTTP/1.1 400"),
                (format!("page={page_id}&url={table}"), "HTTP/1.1 400"),
                (
                    format!("page={}", crate::CrawlerReviewPageId::new()),
                    "HTTP/1.1 404",
                ),
            ] {
                let (status, refused) =
                    send_as(&server, AUTHOR_TOKEN, &format!("{route}?{query}"), "").await;
                assert!(status.starts_with(expected), "{query}: {status} {refused}");
                assert!(refused["error"].is_string(), "{query}: {refused}");
            }
            assert_eq!(
                shop.fetched(),
                [format!("{SHOP}/item/chair"), format!("{SHOP}/item/table")]
            );
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_evaluate_a_proposed_candidate_on_the_review_pages_and_urls_without_storing_it()
         {
            let Fixture {
                pool,
                server,
                shop,
                review_id,
            } = fixture(Shop::default(), Duration::from_secs(5)).await;
            let evaluate = format!("POST /api/reviews/{review_id}/evaluate");
            let proposal =
                |urls: Vec<String>| json!({ "schemas": [schema("h1")], "urls": urls }).to_string();

            let (status, matrix) = send_as(
                &server,
                AUTHOR_TOKEN,
                &evaluate,
                &proposal(vec![format!("{SHOP}/item/table")]),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 200"), "{status} {matrix}");
            let pages = matrix["candidates"][0]["pages"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                pages
                    .iter()
                    .map(|page| (
                        page["page_reference"]["kind"].clone(),
                        page["url"].clone(),
                        page["apply_ok"].clone(),
                        page["extracted"]["title"].clone(),
                    ))
                    .collect::<Vec<_>>(),
                vec![
                    (
                        json!("persisted"),
                        json!(format!("{SHOP}/item/chair")),
                        json!(true),
                        json!("chair")
                    ),
                    (
                        json!("input"),
                        json!(format!("{SHOP}/item/table")),
                        json!(true),
                        json!("table")
                    ),
                ]
            );
            assert_eq!(pages[1]["role"], "EVALUATED_URL");
            assert_eq!(matrix.get("live_evaluation"), None);
            let (version, summary, events): (i64, Value, i64) = sqlx::query_as(
                "SELECT candidate_version, validation_summary, \
                        (SELECT COUNT(*) FROM crawler_review_events e WHERE e.review_id = r.review_id) \
                 FROM crawler_reviews r WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("review: {error}"));
            assert_eq!((version, summary, events), (1, json!({}), 0));

            let fetched_before = shop.fetched().len();
            for (body, refusal) in [
                (
                    proposal(vec!["https://elsewhere.example/item/1".to_owned()]),
                    "is not an http(s) URL",
                ),
                (
                    proposal(
                        (1..=5)
                            .map(|index| format!("{SHOP}/item/{index}"))
                            .collect(),
                    ),
                    "at most 4 URLs",
                ),
                (
                    json!({ "schemas": [] }).to_string(),
                    "invalid ProductSchema review candidate",
                ),
            ] {
                let (status, refused) = send_as(&server, AUTHOR_TOKEN, &evaluate, &body).await;
                assert!(
                    status.starts_with("HTTP/1.1 400"),
                    "{body}: {status} {refused}"
                );
                assert!(
                    refused["error"]
                        .as_str()
                        .is_some_and(|error| error.contains(refusal)),
                    "{body}: {refused}"
                );
            }
            assert_eq!(shop.fetched().len(), fetched_before);
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_a_paced_evaluation_with_accepted_and_its_matrix_on_poll() {
            let Fixture {
                server,
                shop,
                review_id,
                ..
            } = fixture(
                Shop {
                    gate: Some(Semaphore::new(0)),
                    ..Shop::default()
                },
                Duration::from_millis(50),
            )
            .await;

            let (status, accepted) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/evaluate"),
                &json!({ "schemas": [schema("h1")] }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 202"), "{status} {accepted}");
            let evaluation_id = accepted["evaluation_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let poll = format!("GET /api/reviews/{review_id}/evaluations/{evaluation_id}");
            let (status, running) = send_as(&server, REVIEWER_TOKEN, &poll, "").await;
            assert!(status.starts_with("HTTP/1.1 202"), "{status} {running}");

            if let Some(gate) = &shop.gate {
                gate.add_permits(1);
            }
            let mut answer = (String::new(), Value::Null);
            for _ in 0..200 {
                answer = send_as(&server, AUTHOR_TOKEN, &poll, "").await;
                if !answer.0.starts_with("HTTP/1.1 202") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let (status, matrix) = answer;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {matrix}");
            assert_eq!(matrix["candidates"][0]["pages"][0]["apply_ok"], true);

            for path in [
                format!(
                    "/api/reviews/{}/evaluations/{evaluation_id}",
                    CrawlerReviewId::new()
                ),
                format!(
                    "/api/reviews/{review_id}/evaluations/{}",
                    uuid::Uuid::new_v4()
                ),
            ] {
                let (status, unknown) =
                    send_as(&server, AUTHOR_TOKEN, &format!("GET {path}"), "").await;
                assert!(
                    status.starts_with("HTTP/1.1 404"),
                    "{path}: {status} {unknown}"
                );
            }
        }
    }

    /// A page whose last scrape recorded `MissingRequiredValue` opens no review on its own; the
    /// console lists it and adds it to a product-schema review, opened for it when none is
    /// pending, where the reviewer declares a removed-page template the matrix evaluates.
    mod missing_value_pages {
        use super::matrix_refresh::schema;
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send, send_as};
        use super::{ReviewServer, ReviewServerConfig};
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::model::PAGE_ROLE_REVIEWER_ADDED;
        use crate::review::repository::{
            CrawlerReviewRepository, MISSING_REQUIRED_VALUE_REVIEW_REASON,
        };
        use crate::scraper::scraper_service::html_fetcher::{FetchedHtml, MockHtmlFetcher};
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        const NOTICE_PAGE: &str = "https://shop.example/products/sold";
        const NOTICE_HTML: &str = "<html><body><h1></h1><p class=\"notice\">This item is no longer available</p></body></html>";

        /// A console over a ListingSource on `shop.example` with a live schema and one page that
        /// recorded `MissingRequiredValue`, whose fetcher serves that page exactly `fetches` times,
        /// from `redirected_to` when the shop redirects it.
        async fn console(
            fetches: usize,
            redirected_to: Option<&'static str>,
        ) -> (ReviewServer, ListingSourceId) {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            for query in [
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
                  crawl_root_url, crawl_host, created, updated) \
                 VALUES ($1, 'Sold source', 'sold-source', TRUE, 'https://shop.example/', \
                         'shop.example', NOW(), NOW())",
                "INSERT INTO listing_source_urls \
                 (listing_source_id, url, url_class, raw_source_record_key, \
                  last_scraped, last_error_kind, last_error_message) \
                 VALUES ($1, 'https://shop.example/products/sold', 'product', \
                         'https://shop.example/products/sold', NOW(), \
                         'MissingRequiredValue', '`title` is blank')",
            ] {
                sqlx::query(query)
                    .bind(listing_source_id.as_uuid())
                    .execute(&pool)
                    .await
                    .unwrap_or_else(|error| panic!("insert fixture: {error}"));
            }
            sqlx::query(
                "INSERT INTO listing_source_product_schemas \
                 (listing_source_id, product_schema, created, updated) VALUES ($1, $2, NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .bind(json!([schema("h1")]))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert live schemas: {error}"));
            let mut fetcher = MockHtmlFetcher::new();
            fetcher.expect_fetch().times(fetches).returning(move |url| {
                let final_url = redirected_to
                    .map_or_else(|| url.clone(), |target| url::Url::parse(target).unwrap());
                Box::pin(async move {
                    Ok(FetchedHtml {
                        html: NOTICE_HTML.to_owned(),
                        final_url,
                        status: reqwest::StatusCode::OK,
                    })
                })
            });
            let server = ReviewServer::new(
                CrawlerReviewRepository::new(pool),
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    local_mutations_without_token: false,
                    max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
                },
                Arc::new(fetcher),
            );
            (server, listing_source_id)
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_open_a_review_for_a_missing_value_page_and_evaluate_its_template() {
            let (server, listing_source_id) = console(2, None).await;
            let pages_route =
                format!("/api/listing-sources/{listing_source_id}/missing-value-pages");

            let (_, sources) = send(&server, "GET /api/listing_sources", "").await;
            let source = sources
                .as_array()
                .and_then(|sources| {
                    sources
                        .iter()
                        .find(|source| source["listing_source_id"] == listing_source_id.to_string())
                })
                .unwrap_or_else(|| panic!("source not listed: {sources}"));
            assert_eq!(source["missing_value_urls"], 1);
            let (status, listed) = send(&server, &format!("GET {pages_route}"), "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {listed}");
            assert_eq!(listed["pages"][0]["url"], NOTICE_PAGE);
            assert_eq!(listed["pages"][0]["missing"], "`title` is blank");

            let (status, added) = send(
                &server,
                &format!("POST {pages_route}/review"),
                &json!({ "url": NOTICE_PAGE }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 201"), "{status} {added}");
            assert_eq!(added["review_page"]["role"], PAGE_ROLE_REVIEWER_ADDED);
            let review_id = added["review_id"]
                .as_str()
                .unwrap_or_else(|| panic!("no review id: {added}"))
                .to_owned();
            let (_, detail) = send(&server, &format!("GET /api/reviews/{review_id}"), "").await;
            assert_eq!(
                detail["review"]["reason"],
                MISSING_REQUIRED_VALUE_REVIEW_REASON
            );
            assert_eq!(detail["review"]["status"], "PENDING_REVIEW");

            let (status, edited) = send(
                &server,
                &format!("POST /api/reviews/{review_id}/candidate"),
                &json!({
                    "expected_candidate_version": detail["review"]["candidate_version"],
                    "schemas": [schema("h1")],
                    "removed_page_schemas": [{ "selector": "p.notice", "text": "no longer available" }],
                })
                .to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {edited}");
            let (status, matrix) = send(
                &server,
                &format!("GET /api/reviews/{review_id}/matrix?refresh=true"),
                "",
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {matrix}");
            assert_eq!(matrix["removed_page_matches"][0]["url"], NOTICE_PAGE);
            assert_eq!(matrix["removed_page_matches"][0]["outcome"], "REMOVED");
        }

        /// A URL that recorded no missing value is refused before anything is fetched, and the
        /// agents, which administer no ListingSource, cannot open a review this way.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_other_pages_before_fetching_and_agents_altogether() {
            let (server, listing_source_id) = console(0, None).await;
            let route =
                format!("POST /api/listing-sources/{listing_source_id}/missing-value-pages/review");

            let (status, refused) = send(
                &server,
                &route,
                &json!({ "url": "https://shop.example/products/other" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 409"), "{status} {refused}");
            let (status, _) = send_as(
                &server,
                AUTHOR_TOKEN,
                &route,
                &json!({ "url": NOTICE_PAGE }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 401"), "{status}");
        }

        /// A sold page the shop redirects to its homepage is no product page the review takes,
        /// so the request is refused after the fetch and opens no review.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_open_no_review_for_a_page_the_shop_redirects_off_the_product() {
            let (server, listing_source_id) = console(1, Some("https://shop.example/")).await;

            let (status, refused) = send(
                &server,
                &format!(
                    "POST /api/listing-sources/{listing_source_id}/missing-value-pages/review"
                ),
                &json!({ "url": NOTICE_PAGE }).to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 400"), "{status} {refused}");
            let (_, reviews) = send(&server, "GET /api/reviews", "").await;
            assert_eq!(reviews, json!([]), "a refused page opens no review");
        }
    }

    mod mass_removal_confirmation {
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send, send_as};
        use super::{ReviewServer, ReviewServerConfig};
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::{Value, json};
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        async fn console() -> (ReviewServer, ListingSourceId) {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
                  crawl_root_url, crawl_host, created, updated) \
                 VALUES ($1, 'Clearance source', 'clearance-source', TRUE, \
                         'https://shop.example/', 'shop.example', NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert fixture: {error}"));
            let server = ReviewServer::new(
                CrawlerReviewRepository::new(pool),
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    local_mutations_without_token: false,
                    max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
                },
                Arc::new(MockHtmlFetcher::new()),
            );
            (server, listing_source_id)
        }

        async fn overview_of(server: &ReviewServer, listing_source_id: ListingSourceId) -> Value {
            let (_, sources) = send(server, "GET /api/listing_sources", "").await;
            sources
                .as_array()
                .and_then(|sources| {
                    sources
                        .iter()
                        .find(|source| source["listing_source_id"] == listing_source_id.to_string())
                })
                .cloned()
                .unwrap_or_else(|| panic!("source not listed: {sources}"))
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_confirm_and_revoke_a_mass_removal_for_a_human_only() {
            let (server, listing_source_id) = console().await;
            let route =
                format!("/api/listing-sources/{listing_source_id}/mass-removal-confirmation");

            let (status, confirmation) = send(
                &server,
                &format!("POST {route}"),
                &json!({ "hours": 24 }).to_string(),
            )
            .await;
            assert!(
                status.starts_with("HTTP/1.1 200"),
                "{status} {confirmation}"
            );
            assert_eq!(confirmation["mass_removal_confirmed_by"], "HUMAN");
            assert_eq!(confirmation["rearmed_held_urls"], 0);
            let source = overview_of(&server, listing_source_id).await;
            assert_eq!(
                source["mass_removal_confirmed_until"],
                confirmation["mass_removal_confirmed_until"]
            );
            assert_eq!(source["mass_removal_confirmed_by"], "HUMAN");

            let (status, refused) = send(
                &server,
                &format!("POST {route}"),
                &json!({ "hours": 0 }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 400"), "{status} {refused}");
            let (status, _) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST {route}"),
                &json!({ "hours": 24 }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 401"), "{status}");

            let (status, revoked) = send(&server, &format!("DELETE {route}"), "").await;
            assert!(status.starts_with("HTTP/1.1 200"), "{status} {revoked}");
            assert_eq!(revoked, json!({ "ok": true }));
            let source = overview_of(&server, listing_source_id).await;
            assert!(source["mass_removal_confirmed_until"].is_null(), "{source}");
            assert!(source["mass_removal_confirmed_by"].is_null(), "{source}");

            let unknown = format!(
                "/api/listing-sources/{}/mass-removal-confirmation",
                ListingSourceId::new()
            );
            let (status, missing) = send(
                &server,
                &format!("POST {unknown}"),
                &json!({ "hours": 24 }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 404"), "{status} {missing}");
        }
    }
}
