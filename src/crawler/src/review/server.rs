use crate::network::policy::NetworkErrorKind;
use crate::network::robots::MAX_CRAWL_DELAY;
use crate::review::access::ReviewAccess;
pub use crate::review::access::{ReviewServerConfig, ReviewServerConfigError};
use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::candidate_evaluations::CandidateEvaluations;
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::matrix_refresh::{MatrixRefreshOutcome, MatrixRefreshStatus, MatrixRefreshes};
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, CrawlerReviewPage, RawAttributeKey, ReviewActor, ReviewDetail,
    ReviewSchemaMatrix, SampleLabelInput, SchemaMatrix,
};
use crate::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crate::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::schema_evaluation::stored_schema_matrix;
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
/// The most events one answer of `GET /api/review-events` holds; a client reads on from the last
/// event's time, as the review agents' daily digest does.
const MAX_LISTED_REVIEW_EVENTS: usize = 1_000;

mod source_pages;

use source_pages::{EvaluationPayload, parse_review_evaluation_id};

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    domain_administration: Arc<dyn CrawlerDomainAdministration>,
    access: ReviewAccess,
    html_fetcher: Arc<dyn HtmlFetcher>,
    matrix_refreshes: MatrixRefreshes,
    matrix_refresh_wait: Duration,
    candidate_evaluations: CandidateEvaluations,
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
            // The console decides the review agents' approvals and rejections by its settings.
            repository: repository.with_agent_decisions(config.agent_decisions.clone()),
            domain_administration,
            access: ReviewAccess::new(config),
            html_fetcher,
            matrix_refreshes: MatrixRefreshes::default(),
            matrix_refresh_wait: MATRIX_REFRESH_RESPONSE_WAIT,
            candidate_evaluations: CandidateEvaluations::default(),
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
        let agent_decisions = self.access.agent_decisions();
        info!(
            bind_addr = %bind_addr,
            agent_access = self.access.agent_access(),
            agent_approvals = agent_decisions.approvals.as_str(),
            agent_max_decisions_per_hour = agent_decisions.max_decisions_per_hour.get(),
            agent_matrix_max_age_minutes = agent_decisions.max_matrix_age.as_secs() / 60,
            agent_min_labelled_urls = agent_decisions.min_labelled_urls.get(),
            agent_max_repair_rounds = agent_decisions.max_repair_rounds.get(),
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
            return match self.live_preview_url(&request, actor).await {
                Ok(url) => match self.fetch_live_url(&url).await {
                    Ok(html) => HttpResponse::html(200, &instrument_live_html(url.as_str(), &html)),
                    Err(response) => response,
                },
                Err(response) => response,
            }
            .into_review_preview();
        }

        if request.method == "GET" && request.path == "/api/live-html" {
            return match self.live_preview_url(&request, actor).await {
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
            return match self.project_review_source_page(review_id, &request).await {
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
            return self.evaluate_candidate(review_id, payload).await;
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
            // Only a product-schema review has pages to evaluate live; a URL-pattern review is
            // judged on its sample labels.
            if detail.review.artifact_type != ARTIFACT_PRODUCT_SCHEMA {
                return review_error(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    detail.review.artifact_type,
                ));
            }
            if !refresh && let Some(matrix) = cached_schema_matrix(&detail) {
                return HttpResponse::json(200, &ReviewSchemaMatrix::of(matrix, &detail.pages));
            }
            if let Err(error) = self.repository.check_matrix_refresh(review_id, actor).await {
                return review_error(error);
            }
            return self.refresh_schema_matrix(review_id, actor).await;
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
            return match self.repository.get_review(review_id).await {
                Ok(detail) => HttpResponse::json(200, &detail),
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
        tokio::time::timeout(deadline, self.fetch_review_pages(detail.pages))
            .await
            .unwrap_or_else(|_| Err(matrix_refresh_timeout(review_id, deadline)))
    }

    /// Fetches each of `pages` live, one after the other at the shop's pacing.
    async fn fetch_review_pages(
        &self,
        pages: Vec<CrawlerReviewPage>,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let mut fetched = Vec::with_capacity(pages.len());
        for page in pages {
            let url = review_page_url(&page)?;
            let html = self
                .html_fetcher
                .fetch(&url)
                .await
                .map(|fetched| fetched.html)
                .map_err(|err| live_fetch_error(&page, &err))?;
            fetched.push((page, html));
        }
        Ok(fetched)
    }

    /// Starts `actor`'s background refresh, or joins the running one, and answers with the matrix
    /// when it finishes in time, otherwise with `202` for the console to poll.
    async fn refresh_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> HttpResponse {
        let refresh = self.clone().evaluate_live_schema_matrix(review_id, actor);
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

    async fn evaluate_live_schema_matrix(
        self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> MatrixRefreshOutcome {
        let fetched_at = time::OffsetDateTime::now_utc();
        let pages = self.live_review_pages(review_id).await?;
        let evaluated = self
            .repository
            .evaluate_schema_matrix_for_live_pages(review_id, pages, fetched_at)
            .await
            .map_err(review_error)?;
        self.repository
            .store_schema_matrix_if_current(review_id, actor, evaluated)
            .await
            .map_err(review_error)
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
        let fetched = self
            .html_fetcher
            .fetch(&url)
            .await
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))?;
        self.repository
            .add_reviewer_page(review_id, actor, &url, &fetched.final_url, &fetched.html)
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

#[derive(Deserialize)]
struct DomainPayload {
    domain: String,
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

/// The schema matrix cached on the review with the cross-page findings of the current checks,
/// or `None` when it holds none, or one that no longer decodes, so the caller refreshes it from
/// the live pages.
fn cached_schema_matrix(detail: &ReviewDetail) -> Option<SchemaMatrix> {
    stored_schema_matrix(&detail.review.validation_summary)?
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
/// unknown review or review page, 400 for a request the review cannot take, 403 with a `reason`
/// for a change the actor may not make, 409 for a request the review's current state refuses,
/// 429 with `Retry-After` once the review agent made its decisions of the hour, and 500 for a
/// technical failure. A 409 for a request based on an older candidate carries the review's
/// current `candidate_version`, one for an approval of incomplete coverage, unacknowledged or
/// acknowledged at other counts, carries the review's current `coverage`, one for an agent
/// approval the approval guard refuses lists its `failed_checks`, and one for an agent request on
/// a review that waits for a human carries `reason` `HUMAN_REQUIRED`, the agents' `agent_rounds`
/// and the cap as `limit`.
fn review_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::NotFound(_) | ReviewRepositoryError::ReviewPageNotFound(_) => {
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
        | ReviewRepositoryError::InvalidRemovedPageTemplate { .. }
        | ReviewRepositoryError::ReviewPageUrlNotAllowed(_)
        | ReviewRepositoryError::TooManyEvaluatedUrls
        | ReviewRepositoryError::ReviewPageLimitReached(_)
        | ReviewRepositoryError::NotReviewerAddedPage(_)
        | ReviewRepositoryError::RedirectedToNonProductPage(_)
        | ReviewRepositoryError::NoApprovalToRestore(_)
        | ReviewRepositoryError::ApprovalNotRestorable { .. }
        | ReviewRepositoryError::NoRejectionToRevoke(_)
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
        ReviewRepositoryError::AgentDecisionsOff => forbidden(&error, "AGENT_DECISIONS_OFF"),
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
        | ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(_)
        | ReviewRepositoryError::DuplicateReviewPage(_)
        | ReviewRepositoryError::ApprovalAlreadyRestored(_)
        | ReviewRepositoryError::RejectionAlreadyRevoked(_)
        | ReviewRepositoryError::LiveConfigurationChangedSinceApproval(_) => {
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
        | ReviewRepositoryError::Regex(_) => internal_error(error),
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
    use crate::review::agent_decisions::AgentDecisionPolicy;
    use crate::review::approval_guard::FailedApprovalCheck;
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
                agent_tokens: None,
                agent_decisions: AgentDecisionPolicy::default(),
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
                agent_tokens: None,
                agent_decisions: AgentDecisionPolicy::default(),
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
                agent_tokens: None,
                agent_decisions: AgentDecisionPolicy::default(),
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
                agent_tokens: None,
                agent_decisions: AgentDecisionPolicy::default(),
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
                agent_tokens: None,
                agent_decisions: AgentDecisionPolicy::default(),
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
                ReviewRepositoryError::AgentDecisionsOff,
                "AGENT_DECISIONS_OFF",
            ),
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
        use super::{AgentDecisionPolicy, ReviewServer, ReviewServerConfig};
        use crate::network::policy::NetworkErrorKind;
        use crate::review::matrix_refresh::MatrixRefreshStatus;
        use crate::review::model::{PAGE_ROLE_PRIMARY, ReviewActor, SchemaReviewPageInput};
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
                        agent_tokens: None,
                        agent_decisions: AgentDecisionPolicy::default(),
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
    }

    mod needs_repair {
        use super::matrix_refresh::schema;
        use super::{
            AgentDecisionPolicy, RejectingDomainAdministration, ReviewServer, ReviewServerConfig,
        };
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
                Arc::new(RejectingDomainAdministration),
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: None,
                    agent_decisions: AgentDecisionPolicy::default(),
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
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send};
        use super::{
            AgentDecisionPolicy, RejectingDomainAdministration, ReviewServer, ReviewServerConfig,
        };
        use crate::CrawlerReviewId;
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use listing_source_core::ListingSourceId;
        use serde_json::json;
        use std::net::SocketAddr;
        use std::sync::Arc;
        use test_api::IntegrationTestService as _;

        /// A review server over a pending manual schema review, and that review.
        pub(super) async fn pending_review_server() -> (ReviewServer, CrawlerReviewId) {
            pending_review_server_deciding(AgentDecisionPolicy::default()).await
        }

        /// As [`pending_review_server`], taking the agent's decisions by `agent_decisions`.
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
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    agent_decisions,
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
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::schema_editing::pending_review_server_deciding;
        use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_take_the_agent_verdicts_by_the_console_settings() {
            let (off, off_review) =
                pending_review_server_deciding(AgentDecisionPolicy::default()).await;
            let (status, refused) = send_as(
                &off,
                REVIEWER_TOKEN,
                &format!("POST /api/reviews/{off_review}/reject"),
                &json!({ "notes": "not a product page" }).to_string(),
            )
            .await;
            assert!(status.starts_with("HTTP/1.1 403"), "{status} {refused}");
            assert_eq!(refused["reason"], "AGENT_DECISIONS_OFF");

            let (guarded, guarded_review) = pending_review_server_deciding(AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                ..AgentDecisionPolicy::default()
            })
            .await;
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
            let (server, review_id) =
                pending_review_server_deciding(AgentDecisionPolicy::default()).await;
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
            let (server, review_id) = pending_review_server_deciding(AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                ..AgentDecisionPolicy::default()
            })
            .await;
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
        /// say the review waits for a human, every agent route on it answers 409
        /// `HUMAN_REQUIRED` before any page is fetched and records nothing, and a human's verdict
        /// hands it back.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_answer_human_required_on_every_agent_route_once_the_repair_cap_is_reached()
        {
            let (server, review_id) = pending_review_server_deciding(AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                max_repair_rounds: std::num::NonZeroU32::MIN,
                ..AgentDecisionPolicy::default()
            })
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
        use super::schema_editing::pending_review_server_deciding;
        use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
        use serde_json::json;
        use test_api::IntegrationTestService as _;

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_each_role_what_the_other_role_does_with_its_reason() {
            let (server, review_id) = pending_review_server_deciding(AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                ..AgentDecisionPolicy::default()
            })
            .await;
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
        use super::{RejectingDomainAdministration, ReviewServer, ReviewServerConfig};
        use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
        use crate::review::agent_tokens::ReviewAgentTokens;
        use crate::review::model::UrlPatternDecision;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use crate::{CrawlerDomainId, CrawlerReviewId};
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

        /// A review server taking the agents' guarded decisions, and the domain and the pending,
        /// undecided URL-pattern review of the shop on [`SHOP`], whose ListingSource also holds
        /// `other_domains`.
        async fn url_pattern_review_server(
            pool: &PgPool,
            other_domains: &[&str],
        ) -> (ReviewServer, CrawlerDomainId, CrawlerReviewId) {
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
            let domain_id = CrawlerDomainId::new();
            let others = other_domains
                .iter()
                .map(|domain| (CrawlerDomainId::new(), *domain));
            for (id, domain) in std::iter::once((domain_id, "antique-shop.example")).chain(others) {
                sqlx::query(
                    "INSERT INTO listing_source_domains \
                     (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
                     VALUES ($1, $2, $3, $3)",
                )
                .bind(id.as_uuid())
                .bind(listing_source_id.as_uuid())
                .bind(domain)
                .execute(pool)
                .await
                .unwrap_or_else(|error| panic!("insert domain {domain}: {error}"));
            }
            let repository = CrawlerReviewRepository::new(pool.clone());
            let review_id = repository
                .create_url_pattern_review(
                    &listing_source_id,
                    &domain_id,
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
                Arc::new(RejectingDomainAdministration),
                ReviewServerConfig {
                    bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                    auth_token: Some(TOKEN.to_owned()),
                    agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                    agent_decisions: AgentDecisionPolicy {
                        approvals: AgentApprovals::Guarded,
                        ..AgentDecisionPolicy::default()
                    },
                    local_mutations_without_token: false,
                },
                Arc::new(MockHtmlFetcher::new()),
            );
            (server, domain_id, review_id)
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_take_the_reviewer_approval_once_the_pattern_follows_the_author_labels() {
            let pool = test_api::get_postgres_client().await;
            let (server, domain_id, review_id) = url_pattern_review_server(&pool, &[]).await;
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
                "SELECT url_pattern, url_pattern_state FROM listing_source_domains \
                 WHERE domain_id = $1",
            )
            .bind(domain_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read domain decision: {error}"));
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

        /// A URL-pattern review decides one domain, so a label of a URL on another domain of its
        /// ListingSource, which would say nothing about the decision, is refused with 400 and
        /// changes nothing.
        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_refuse_a_label_on_another_domain_of_the_listing_source() {
            let pool = test_api::get_postgres_client().await;
            let (server, _, review_id) =
                url_pattern_review_server(&pool, &["antique-shop.de"]).await;
            let other_domain = "https://antique-shop.de/item/oak-chair";
            let mut off_domain = labels(&PRODUCTS, &NOT_PRODUCTS);
            off_domain[0]["url"] = json!(other_domain);

            let (status, refused) = send_as(
                &server,
                AUTHOR_TOKEN,
                &format!("POST /api/reviews/{review_id}/sample-labels"),
                &json!({ "expected_candidate_version": 1, "labels": off_domain }).to_string(),
            )
            .await;

            assert!(status.starts_with("HTTP/1.1 400"), "{status} {refused}");
            assert!(
                refused["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(other_domain)),
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

    /// The console's page fetches for a review: a review agent's live previews stay on the
    /// review's ListingSource, a page is projected as the schema prompt sees it, and a proposed
    /// candidate is evaluated on the review's pages and on URLs of its ListingSource without being
    /// stored.
    mod review_source_pages {
        use super::matrix_refresh::schema;
        use super::needs_repair::{AUTHOR_TOKEN, REVIEWER_TOKEN, TOKEN, send_as};
        use super::{
            AgentDecisionPolicy, RejectingDomainAdministration, ReviewServer, ReviewServerConfig,
        };
        use crate::CrawlerReviewId;
        use crate::network::policy::NetworkErrorKind;
        use crate::review::agent_tokens::ReviewAgentTokens;
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
            sqlx::query(
                "INSERT INTO listing_source_domains \
                 (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
                 VALUES ($1, $2, 'antiques.example', 'antiques.example')",
            )
            .bind(crate::CrawlerDomainId::new().as_uuid())
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert domain: {error}"));
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
                    Arc::new(RejectingDomainAdministration),
                    ReviewServerConfig {
                        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                        auth_token: Some(TOKEN.to_owned()),
                        agent_tokens: Some(ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)),
                        agent_decisions: AgentDecisionPolicy::default(),
                        local_mutations_without_token: false,
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

        /// Sends a request with `token` as its bearer and returns its status line and body.
        async fn send_raw(server: &ReviewServer, token: &str, request: &str) -> (String, String) {
            let response = server
                .route(&format!(
                    "{request} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n"
                ))
                .await
                .to_string();
            let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
            (
                head.lines().next().unwrap_or_default().to_owned(),
                body.to_owned(),
            )
        }

        fn encoded(url: &str) -> String {
            url::form_urlencoded::byte_serialize(url.as_bytes()).collect()
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_fetch_an_agent_live_preview_only_of_the_named_review_source() {
            let Fixture {
                server,
                shop,
                review_id,
                ..
            } = fixture(Shop::default(), Duration::from_secs(5)).await;
            let foreign = encoded("https://elsewhere.example/item/chair");
            let own = encoded(&format!("{SHOP}/item/table"));

            for route in ["/api/live-html", "/api/live-inspect"] {
                let fetched_before = shop.fetched().len();
                for (token, query, status) in [
                    (AUTHOR_TOKEN, format!("url={own}"), "HTTP/1.1 400"),
                    (
                        REVIEWER_TOKEN,
                        format!("url={foreign}&review={review_id}"),
                        "HTTP/1.1 400",
                    ),
                    (
                        AUTHOR_TOKEN,
                        format!("url={own}&review={}", CrawlerReviewId::new()),
                        "HTTP/1.1 404",
                    ),
                ] {
                    let (line, body) =
                        send_raw(&server, token, &format!("GET {route}?{query}")).await;
                    assert!(line.starts_with(status), "{route}?{query}: {line} {body}");
                }
                assert_eq!(
                    shop.fetched().len(),
                    fetched_before,
                    "{route}: {:?}",
                    shop.fetched()
                );

                let (line, body) = send_raw(
                    &server,
                    REVIEWER_TOKEN,
                    &format!("GET {route}?url={own}&review={review_id}"),
                )
                .await;
                assert!(line.starts_with("HTTP/1.1 200"), "{route}: {line} {body}");
                let (line, body) =
                    send_raw(&server, TOKEN, &format!("GET {route}?url={foreign}")).await;
                assert!(line.starts_with("HTTP/1.1 200"), "{route}: {line} {body}");
            }
            assert_eq!(
                shop.fetched(),
                [
                    format!("{SHOP}/item/table"),
                    "https://elsewhere.example/item/chair".to_owned(),
                    format!("{SHOP}/item/table"),
                    "https://elsewhere.example/item/chair".to_owned(),
                ]
            );
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
}
