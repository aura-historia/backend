use crate::review::repository::CrawlerReviewRepository;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
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

    fn authorized(&self, headers: &HashMap<String, String>) -> bool {
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

fn parse_action_payload(body: &str) -> ActionPayload {
    serde_json::from_str(body).unwrap_or_default()
}

struct ParsedRequest<'a> {
    method: String,
    path: &'a str,
    headers: HashMap<String, String>,
    body: &'a str,
}

fn parse_request(request: &str) -> Option<ParsedRequest<'_>> {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_path = parts.next()?;
    let path = raw_path.split('?').next().unwrap_or(raw_path);

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Some(ParsedRequest {
        method,
        path,
        headers,
        body,
    })
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

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl HttpResponse {
    fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn html(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            body: body.to_string(),
        }
    }

    fn json(status: u16, body: &impl serde::Serialize) -> Self {
        let body = serde_json::to_string_pretty(body)
            .unwrap_or_else(|_| "{\"error\":\"failed to serialize response\"}".to_string());
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body,
        }
    }
}

impl std::fmt::Display for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "OK",
        };
        write!(
            f,
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n{}",
            self.status,
            reason,
            self.content_type,
            self.body.len(),
            self.body
        )
    }
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Crawler Review Console</title>
  <style>
    body { margin: 0; font-family: system-ui, sans-serif; background: #f7f8fa; color: #172033; }
    header { padding: 16px 24px; background: #172033; color: white; }
    main { display: grid; grid-template-columns: 320px 1fr; gap: 16px; padding: 16px; }
    section { background: white; border: 1px solid #d8dee8; border-radius: 8px; overflow: hidden; }
    h1 { font-size: 18px; margin: 0; }
    h2 { font-size: 14px; margin: 0; padding: 12px 14px; border-bottom: 1px solid #d8dee8; }
    button { border: 1px solid #b8c2d1; background: white; padding: 7px 10px; border-radius: 6px; cursor: pointer; }
    button.primary { background: #1463ff; border-color: #1463ff; color: white; }
    button.danger { background: #fff4f4; color: #b42318; border-color: #f2b8b5; }
    .list { max-height: calc(100vh - 130px); overflow: auto; }
    .item { padding: 10px 12px; border-bottom: 1px solid #edf0f5; cursor: pointer; }
    .item:hover, .item.active { background: #eef4ff; }
    .muted { color: #667085; font-size: 12px; }
    .pill { display: inline-block; padding: 2px 7px; border-radius: 999px; font-size: 11px; background: #eef2f6; margin-left: 6px; }
    .detail { padding: 14px; }
    pre, textarea { width: 100%; box-sizing: border-box; border: 1px solid #d8dee8; border-radius: 6px; background: #fbfcfe; padding: 10px; overflow: auto; }
    textarea { min-height: 240px; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; }
    iframe { width: 100%; min-height: 420px; border: 1px solid #d8dee8; border-radius: 6px; background: white; }
    table { width: 100%; border-collapse: collapse; font-size: 13px; }
    th, td { border-bottom: 1px solid #edf0f5; padding: 7px; text-align: left; vertical-align: top; }
    .actions { display: flex; gap: 8px; margin: 12px 0; flex-wrap: wrap; }
  </style>
</head>
<body>
  <header><h1>Crawler Review Console</h1><div class="muted">Localhost/SSH internal UI</div></header>
  <main>
    <section>
      <h2>Pending Reviews</h2>
      <div id="reviewList" class="list"></div>
    </section>
    <section>
      <h2>Review Detail</h2>
      <div id="detail" class="detail muted">Select a review.</div>
    </section>
  </main>
  <script>
    let selectedId = null;
    async function api(path, options = {}) {
      const res = await fetch(path, { headers: { 'content-type': 'application/json' }, ...options });
      if (!res.ok) throw new Error(await res.text());
      return res.json();
    }
    async function loadReviews() {
      const reviews = await api('/api/reviews');
      const list = document.getElementById('reviewList');
      list.innerHTML = reviews.map(r => `
        <div class="item ${r.review_id === selectedId ? 'active' : ''}" onclick="selectReview('${r.review_id}')">
          <strong>${r.artifact_type}</strong><span class="pill">${r.status}</span>
          <div class="muted">${r.reason}</div>
          <div class="muted">${r.shop_id}</div>
        </div>`).join('');
    }
    async function selectReview(id) {
      selectedId = id;
      await loadReviews();
      const detail = await api(`/api/reviews/${id}`);
      const matrix = detail.review.artifact_type === 'PRODUCT_SCHEMA'
        ? await api(`/api/reviews/${id}/matrix`)
        : null;
      renderDetail(detail, matrix);
    }
    function renderDetail(detail, matrix) {
      const review = detail.review;
      const pages = detail.pages || [];
      const urls = detail.urls || [];
      document.getElementById('detail').innerHTML = `
        <div><strong>${review.artifact_type}</strong><span class="pill">${review.status}</span></div>
        <div class="muted">${review.review_id}</div>
        <div class="actions">
          <button onclick="triggerAction('trigger-crawl')">Trigger crawl</button>
          <button onclick="triggerAction('trigger-scrape')">Trigger scrape</button>
          <button onclick="triggerAction('regenerate-pattern')">Regenerate pattern</button>
          <button onclick="triggerAction('regenerate-schema')">Regenerate schema</button>
          <button class="primary" onclick="approveReview()">Approve</button>
          <button class="danger" onclick="rejectReview()">Reject</button>
          <button onclick="needsRepair()">Needs repair</button>
          <button onclick="saveCandidate()">Save edited candidate</button>
        </div>
        <h3>Candidate Payload</h3>
        <textarea id="candidatePayload">${escapeHtml(JSON.stringify(review.candidate_payload, null, 2))}</textarea>
        <h3>Validation</h3>
        <pre>${escapeHtml(JSON.stringify(review.validation_summary, null, 2))}</pre>
        ${urls.length ? renderUrls(urls) : ''}
        ${matrix ? renderMatrix(matrix) : ''}
        ${pages.length ? renderPages(pages) : ''}
      `;
    }
    function renderUrls(urls) {
      return `<h3>URL Pattern Preview</h3><table><thead><tr><th>URL</th><th>Current</th><th>Candidate</th><th>Class</th></tr></thead><tbody>${
        urls.map(u => `<tr><td>${escapeHtml(u.url)}</td><td>${u.current_pattern_match}</td><td>${u.candidate_pattern_match}</td><td>${u.candidate_class}</td></tr>`).join('')
      }</tbody></table>`;
    }
    function renderMatrix(matrix) {
      return `<h3>Schema Matrix</h3><pre>${escapeHtml(JSON.stringify(matrix, null, 2))}</pre>`;
    }
    function renderPages(pages) {
      const first = pages[0];
      return `<h3>Snapshot</h3><div class="muted">${escapeHtml(first.url)} (${first.role})</div><iframe src="/api/review-pages/${first.review_page_id}/html" sandbox></iframe>`;
    }
    async function approveReview() {
      await api(`/api/reviews/${selectedId}/approve`, { method: 'POST', body: JSON.stringify({ notes: prompt('Notes') || null }) });
      await loadReviews(); await selectReview(selectedId);
    }
    async function rejectReview() {
      await api(`/api/reviews/${selectedId}/reject`, { method: 'POST', body: JSON.stringify({ notes: prompt('Notes') || null }) });
      await loadReviews(); await selectReview(selectedId);
    }
    async function needsRepair() {
      await api(`/api/reviews/${selectedId}/needs-repair`, { method: 'POST', body: JSON.stringify({ notes: prompt('Repair notes') || null }) });
      await loadReviews(); await selectReview(selectedId);
    }
    async function saveCandidate() {
      const payload = JSON.parse(document.getElementById('candidatePayload').value);
      await api(`/api/reviews/${selectedId}/candidate`, { method: 'POST', body: JSON.stringify(payload) });
      await selectReview(selectedId);
    }
    async function triggerAction(action) {
      const detail = await api(`/api/reviews/${selectedId}`);
      const shopId = detail.review.shop_id;
      const result = await api(`/api/shops/${shopId}/${action}`, { method: 'POST', body: '{}' });
      alert(`${action}: ${result.affected} rows affected`);
    }
    function escapeHtml(s) {
      return String(s).replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
    }
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
  </script>
</body>
</html>"#;
