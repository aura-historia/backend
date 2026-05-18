use crate::review::assets::{APP_JS, INDEX_HTML, STYLES_CSS, instrument_review_page};
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::repository::CrawlerReviewRepository;
use crate::review::repository::ReviewRepositoryError;
use crate::scraper::css_selector::rule::ExtractionRule;
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
}

impl ReviewServerConfig {
    pub fn from_env() -> Result<Self, std::net::AddrParseError> {
        let bind_addr = std::env::var("CRAWLER_REVIEW_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
            .parse()?;
        let auth_token = std::env::var("CRAWLER_REVIEW_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        Ok(Self {
            bind_addr,
            auth_token,
        })
    }
}

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    config: ReviewServerConfig,
}

impl ReviewServer {
    pub fn new(repository: CrawlerReviewRepository, config: ReviewServerConfig) -> Self {
        Self { repository, config }
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
                    warn!(peer = %peer, error = %err, "Review console request failed");
                }
            });
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let mut buffer = vec![0; 1024 * 1024];
        let bytes_read = stream.read(&mut buffer).await?;
        if bytes_read == 0 {
            return Ok(());
        }
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);
        let response = self.route(&request).await.to_string();
        stream.write_all(response.as_bytes()).await?;
        stream.shutdown().await
    }

    async fn route(&self, request: &str) -> HttpResponse {
        let Some(parsed) = parse_request(request) else {
            return HttpResponse::text(400, "bad request");
        };

        if parsed.path.starts_with("/api/") && !self.authorized(&parsed.headers) {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        }

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/api/shops") => match self.repository.list_shops(200).await {
                Ok(shops) => HttpResponse::json(200, &shops),
                Err(err) => internal_error(err),
            },
            ("GET", "/api/reviews") => match self.repository.list_reviews(200).await {
                Ok(reviews) => HttpResponse::json(200, &reviews),
                Err(err) => internal_error(err),
            },
            _ => self.route_dynamic(parsed).await,
        }
    }

    async fn route_dynamic(&self, request: ParsedRequest<'_>) -> HttpResponse {
        if request.method == "GET"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/matrix")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/matrix") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            return match self.repository.evaluate_schema_matrix(review_id).await {
                Ok(matrix) => HttpResponse::json(200, &matrix),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/inspect")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/inspect") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.repository.get_review_page(page_id).await {
                Ok(Some(page)) => HttpResponse::html(200, &instrument_review_page(&page)),
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/html")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/html") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.repository.get_review_page_html(page_id).await {
                Ok(Some(html)) => HttpResponse::html(200, &html),
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(err) => internal_error(err),
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
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/shops/")
            && request.path.ends_with("/trigger-crawl")
        {
            let Some(shop_id) = parse_shop_id_with_suffix(request.path, "/trigger-crawl") else {
                return HttpResponse::json(400, &json!({ "error": "invalid shop id" }));
            };
            return match self.repository.trigger_crawl_now(shop_id).await {
                Ok(rows) => HttpResponse::json(200, &json!({ "ok": true, "affected": rows })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/shops/")
            && request.path.ends_with("/trigger-scrape")
        {
            let Some(shop_id) = parse_shop_id_with_suffix(request.path, "/trigger-scrape") else {
                return HttpResponse::json(400, &json!({ "error": "invalid shop id" }));
            };
            return match self.repository.trigger_scrape_now(shop_id).await {
                Ok(rows) => HttpResponse::json(200, &json!({ "ok": true, "affected": rows })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/shops/")
            && request.path.ends_with("/regenerate-pattern")
        {
            let Some(shop_id) = parse_shop_id_with_suffix(request.path, "/regenerate-pattern")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid shop id" }));
            };
            return match self
                .repository
                .trigger_url_pattern_regeneration(shop_id)
                .await
            {
                Ok(rows) => HttpResponse::json(200, &json!({ "ok": true, "affected": rows })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/shops/")
            && request.path.ends_with("/regenerate-schema")
        {
            let Some(shop_id) = parse_shop_id_with_suffix(request.path, "/regenerate-schema")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid shop id" }));
            };
            return match self.repository.trigger_schema_regeneration(shop_id).await {
                Ok(rows) => HttpResponse::json(200, &json!({ "ok": true, "affected": rows })),
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
            .is_some_and(|actual| actual == expected)
    }
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

fn parse_action_payload(body: &str) -> ActionPayload {
    serde_json::from_str(body).unwrap_or_default()
}

fn parse_review_id(path: &str) -> Option<uuid::Uuid> {
    let id = path.strip_prefix("/api/reviews/")?;
    uuid::Uuid::parse_str(id).ok()
}

fn parse_review_id_with_suffix(path: &str, suffix: &str) -> Option<uuid::Uuid> {
    let without_suffix = path.strip_suffix(suffix)?;
    parse_review_id(without_suffix)
}

fn parse_page_id_with_suffix(path: &str, suffix: &str) -> Option<uuid::Uuid> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/review-pages/")?;
    uuid::Uuid::parse_str(id).ok()
}

fn parse_shop_id_with_suffix(path: &str, suffix: &str) -> Option<common::shop_id::ShopId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/shops/")?;
    uuid::Uuid::parse_str(id)
        .ok()
        .map(common::shop_id::ShopId::from)
}

fn internal_error(error: impl std::fmt::Display) -> HttpResponse {
    error!(error = %error, "Review API error");
    HttpResponse::json(500, &json!({ "error": error.to_string() }))
}
