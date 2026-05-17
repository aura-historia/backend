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

fn instrument_review_page(page: &crate::review::repository::CrawlerReviewPage) -> String {
    let raw_html = disable_existing_scripts(&page.raw_html);
    let base = format!(
        r#"<base href="{}"><style>{}</style>"#,
        escape_html_attr(&page.url),
        INSPECTOR_CSS
    );
    let script = format!("<script>{INSPECTOR_JS}</script>");

    let with_head = if raw_html.contains("<head>") {
        raw_html.replacen("<head>", &format!("<head>{base}"), 1)
    } else if raw_html.contains("<HEAD>") {
        raw_html.replacen("<HEAD>", &format!("<HEAD>{base}"), 1)
    } else {
        format!("{base}{raw_html}")
    };

    if with_head.contains("</body>") {
        with_head.replacen("</body>", &format!("{script}</body>"), 1)
    } else if with_head.contains("</BODY>") {
        with_head.replacen("</BODY>", &format!("{script}</BODY>"), 1)
    } else {
        format!("{with_head}{script}")
    }
}

fn disable_existing_scripts(html: &str) -> String {
    html.replace(
        "<script",
        r#"<script type="text/plain" data-crawler-review-disabled="true""#,
    )
    .replace(
        "<SCRIPT",
        r#"<SCRIPT type="text/plain" data-crawler-review-disabled="true""#,
    )
}

fn escape_html_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

const INSPECTOR_CSS: &str = r#"
.__crawler_review_hover { outline: 3px solid #f59e0b !important; outline-offset: 2px !important; cursor: crosshair !important; }
.__crawler_review_selected { outline: 3px solid #2563eb !important; outline-offset: 2px !important; background-color: rgba(37, 99, 235, 0.10) !important; }
"#;

const INSPECTOR_JS: &str = r#"
(() => {
  const selected = new Set();
  let hover = null;

  function cssEscape(value) {
    if (window.CSS && CSS.escape) return CSS.escape(value);
    return String(value).replace(/[^a-zA-Z0-9_-]/g, ch => '\\' + ch);
  }

  function selectorFor(element) {
    if (!element || element.nodeType !== Node.ELEMENT_NODE) return '';
    if (element.id) return `#${cssEscape(element.id)}`;

    const parts = [];
    let current = element;
    while (current && current.nodeType === Node.ELEMENT_NODE && current !== document.body) {
      let part = current.localName.toLowerCase();
      const stableClasses = Array.from(current.classList || [])
        .filter(name => !name.startsWith('__crawler_review_'))
        .slice(0, 2);
      if (stableClasses.length) part += stableClasses.map(name => `.${cssEscape(name)}`).join('');

      const parent = current.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter(child => child.localName === current.localName);
        if (siblings.length > 1 && !stableClasses.length) {
          part += `:nth-of-type(${siblings.indexOf(current) + 1})`;
        }
      }
      parts.unshift(part);
      const selector = parts.join(' > ');
      try {
        if (document.querySelectorAll(selector).length === 1) return selector;
      } catch (_) {}
      current = current.parentElement;
    }
    return parts.join(' > ');
  }

  function clearSelected() {
    for (const element of selected) element.classList.remove('__crawler_review_selected');
    selected.clear();
  }

  function highlight(selector) {
    clearSelected();
    if (!selector) return;
    try {
      document.querySelectorAll(selector).forEach(element => {
        element.classList.add('__crawler_review_selected');
        selected.add(element);
      });
    } catch (_) {}
  }

  document.addEventListener('mouseover', event => {
    if (hover) hover.classList.remove('__crawler_review_hover');
    hover = event.target;
    hover.classList.add('__crawler_review_hover');
  }, true);

  document.addEventListener('mouseout', () => {
    if (hover) hover.classList.remove('__crawler_review_hover');
    hover = null;
  }, true);

  document.addEventListener('click', event => {
    event.preventDefault();
    event.stopPropagation();
    const selector = selectorFor(event.target);
    highlight(selector);
    window.parent.postMessage({
      type: 'crawler-review-selector-picked',
      selector,
      text: (event.target.innerText || event.target.textContent || '').trim().slice(0, 500),
      tag: event.target.localName,
      href: event.target.getAttribute && event.target.getAttribute('href'),
      src: event.target.getAttribute && event.target.getAttribute('src')
    }, '*');
  }, true);

  window.addEventListener('message', event => {
    if (event.data && event.data.type === 'crawler-review-highlight-selector') {
      highlight(event.data.selector);
    }
  });
})();
"#;

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Crawler Review Console</title>
  <style>
    :root { --ink: #182230; --muted: #667085; --line: #d8dee8; --soft: #f3f5f8; --accent: #2563eb; --danger: #b42318; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: Inter, ui-sans-serif, system-ui, sans-serif; background: #f7f8fa; color: var(--ink); }
    header { display: flex; align-items: center; justify-content: space-between; padding: 12px 18px; background: #111827; color: white; }
    main { display: grid; grid-template-columns: 300px minmax(0, 1fr); gap: 12px; padding: 12px; height: calc(100vh - 57px); }
    section { background: white; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; min-width: 0; }
    h1 { font-size: 17px; margin: 0; }
    h2 { font-size: 13px; margin: 0; padding: 10px 12px; border-bottom: 1px solid var(--line); }
    h3 { font-size: 13px; margin: 14px 0 8px; }
    button, select, input { border: 1px solid #b8c2d1; background: white; min-height: 32px; padding: 6px 9px; border-radius: 6px; }
    button { cursor: pointer; }
    button.primary { background: var(--accent); border-color: var(--accent); color: white; }
    button.danger { background: #fff4f4; color: var(--danger); border-color: #f2b8b5; }
    label { display: grid; gap: 4px; font-size: 12px; color: var(--muted); }
    .list { max-height: 100%; overflow: auto; }
    .item { padding: 10px 12px; border-bottom: 1px solid #edf0f5; cursor: pointer; }
    .item:hover, .item.active { background: #eef4ff; }
    .muted { color: var(--muted); font-size: 12px; }
    .pill { display: inline-block; padding: 2px 7px; border-radius: 999px; font-size: 11px; background: #eef2f6; margin-left: 6px; }
    .detail { height: 100%; overflow: auto; padding: 12px; }
    .actions, .toolbar { display: flex; gap: 8px; margin: 10px 0; flex-wrap: wrap; align-items: end; }
    .schema-workbench { display: grid; grid-template-columns: minmax(420px, 1.25fr) minmax(360px, .75fr); gap: 12px; align-items: stretch; }
    .preview-panel, .editor-panel { border: 1px solid var(--line); border-radius: 8px; overflow: hidden; background: #fff; min-width: 0; }
    .panel-head { display: flex; justify-content: space-between; gap: 8px; padding: 9px 10px; border-bottom: 1px solid var(--line); background: #fbfcfe; }
    iframe { width: 100%; height: 70vh; border: 0; background: white; display: block; }
    pre, textarea { width: 100%; border: 1px solid var(--line); border-radius: 6px; background: #fbfcfe; padding: 10px; overflow: auto; }
    textarea { min-height: 220px; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-size: 12px; }
    table { width: 100%; border-collapse: collapse; font-size: 12px; }
    th, td { border-bottom: 1px solid #edf0f5; padding: 6px; text-align: left; vertical-align: top; }
    tr.failed { background: #fff7ed; }
    .status-ok { color: #067647; font-weight: 600; }
    .status-failed { color: #b42318; font-weight: 600; }
    .error-panel { margin: 8px 0 10px; padding: 9px 10px; border: 1px solid #f2b8b5; border-radius: 6px; background: #fff4f4; color: #7a271a; font-size: 12px; }
    .error-panel strong { display: block; margin-bottom: 4px; color: #b42318; }
    .selector-box { display: grid; grid-template-columns: 1fr auto; gap: 8px; }
    .field-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    .picked { padding: 8px; background: var(--soft); border-radius: 6px; min-height: 48px; }
    @media (max-width: 980px) { main, .schema-workbench { grid-template-columns: 1fr; height: auto; } iframe { height: 58vh; } }
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
    let selectedDetail = null;
    let selectedMatrix = null;
    let selectedSchemaIndex = 0;
    let selectedField = 'title';
    let selectedPageId = null;
    const selectorFields = ['shops_product_id','title','description','price','price_estimate_min','price_estimate_max','state','images','auction_start','auction_end'];

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
          <div class="muted">${escapeHtml(r.shop_name || r.shop_id)}</div>
        </div>`).join('');
    }
    async function selectReview(id) {
      selectedId = id;
      await loadReviews();
      const detail = await api(`/api/reviews/${id}`);
      const matrix = detail.review.artifact_type === 'PRODUCT_SCHEMA'
        ? await api(`/api/reviews/${id}/matrix`)
        : null;
      selectedDetail = detail;
      selectedMatrix = matrix;
      selectedSchemaIndex = 0;
      selectedField = 'title';
      selectedPageId = (detail.pages && detail.pages[0] && detail.pages[0].review_page_id) || null;
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
        ${matrix ? '' : `
          <h3>Candidate Payload</h3>
          <textarea id="candidatePayload">${escapeHtml(JSON.stringify(review.candidate_payload, null, 2))}</textarea>
        `}
        <h3>Validation</h3>
        <pre>${escapeHtml(JSON.stringify(review.validation_summary, null, 2))}</pre>
        ${urls.length ? renderUrls(urls) : ''}
        ${matrix ? renderSchemaWorkbench(detail, matrix) : ''}
      `;
      if (matrix) postHighlightSoon();
    }
    function renderUrls(urls) {
      return `<h3>URL Pattern Preview</h3><table><thead><tr><th>URL</th><th>Current</th><th>Candidate</th><th>Class</th></tr></thead><tbody>${
        urls.map(u => `<tr><td>${escapeHtml(u.url)}</td><td>${u.current_pattern_match}</td><td>${u.candidate_pattern_match}</td><td>${u.candidate_class}</td></tr>`).join('')
      }</tbody></table>`;
    }
    function renderMatrix(matrix) {
      return `<h3>Schema Matrix</h3><pre>${escapeHtml(JSON.stringify(matrix, null, 2))}</pre>`;
    }
    function renderSchemaWorkbench(detail, matrix) {
      const pages = detail.pages || [];
      const schemas = (((detail.review || {}).candidate_payload || {}).schemas) || [];
      const page = pages.find(p => p.review_page_id === selectedPageId) || pages[0];
      if (page && !selectedPageId) selectedPageId = page.review_page_id;
      return `
        <h3>Schema Workbench</h3>
        <div class="schema-workbench">
          <div class="preview-panel">
            <div class="panel-head">
              <div>
                <strong>Product View</strong>
                <div class="muted">${page ? `${escapeHtml(page.url)} (${page.role})` : 'No saved page snapshot'}</div>
              </div>
              <div class="toolbar">
                <select onchange="selectedPageId=this.value; rerenderWorkbench()">
                  ${pages.map(p => `<option value="${p.review_page_id}" ${p.review_page_id === selectedPageId ? 'selected' : ''}>${p.role}</option>`).join('')}
                </select>
              </div>
            </div>
            ${page ? `<iframe id="snapshotFrame" src="/api/review-pages/${page.review_page_id}/inspect" sandbox="allow-scripts"></iframe>` : ''}
          </div>
          <div class="editor-panel">
            <div class="panel-head">
              <strong>Selector Editor</strong>
              <span class="muted">Click an element in the product view</span>
            </div>
            <div style="padding:10px">
              <div class="field-grid">
                <label>Schema
                  <select onchange="selectedSchemaIndex=Number(this.value); rerenderWorkbench()">
                    ${schemas.map((_, i) => `<option value="${i}" ${i === selectedSchemaIndex ? 'selected' : ''}>Schema ${i + 1} / ${schemas.length}</option>`).join('')}
                  </select>
                </label>
                <label>Field
                  <select onchange="selectedField=this.value; rerenderWorkbench()">
                    ${selectorFields.map(field => `<option value="${field}" ${field === selectedField ? 'selected' : ''}>${field}</option>`).join('')}
                  </select>
                </label>
              </div>
              <h3>Selected Selector</h3>
              <div class="selector-box">
                <input id="selectedSelector" value="${escapeHtmlAttr(currentSelector())}" oninput="setCurrentSelector(this.value); postHighlight(this.value)">
                <button onclick="saveCandidate()">Save</button>
              </div>
              <div class="actions">
                <button onclick="applyPickedSelector()">Use clicked element</button>
                <button onclick="postHighlight(currentSelector())">Highlight selector</button>
                <button onclick="reloadMatrix()">Re-evaluate</button>
              </div>
              <div class="picked" id="pickedElement">No element picked yet.</div>
              <h3>Extracted Data</h3>
              ${renderApplyErrors(matrix)}
              ${renderExtractedRows(matrix)}
              <h3>Full JSON</h3>
              <textarea id="candidatePayload">${escapeHtml(JSON.stringify(detail.review.candidate_payload, null, 2))}</textarea>
            </div>
          </div>
        </div>`;
    }
    function renderApplyErrors(matrix) {
      const candidate = matrix.candidates.find(c => c.schema_index === selectedSchemaIndex);
      if (!candidate) return '';
      const failures = candidate.pages.filter(page => !page.apply_ok);
      if (!failures.length) return '';
      return `<div class="error-panel">
        <strong>Schema ${selectedSchemaIndex + 1} failed on ${failures.length} saved page${failures.length === 1 ? '' : 's'}</strong>
        ${failures.map(page => `<div>${escapeHtml(page.role)}: ${escapeHtml(page.error || 'Schema did not apply.')}</div>`).join('')}
      </div>`;
    }
    function renderExtractedRows(matrix) {
      const candidate = matrix.candidates.find(c => c.schema_index === selectedSchemaIndex);
      if (!candidate) return '<div class="muted">No extracted data for this schema.</div>';
      return `<table><thead><tr><th>Page</th><th>Status</th><th>ID</th><th>Title</th><th>Price</th><th>State</th><th>Images</th><th>Description</th><th>Error</th></tr></thead><tbody>${
        candidate.pages.map(page => {
          const raw = page.extracted || {};
          return `
          <tr class="${page.apply_ok ? '' : 'failed'}">
            <td>${escapeHtml(page.role)}</td>
            <td class="${page.apply_ok ? 'status-ok' : 'status-failed'}">${page.apply_ok ? 'OK' : 'Failed'}</td>
            <td>${escapeHtml(raw.shops_product_id || '')}</td>
            <td>${escapeHtml(raw.title || '')}</td>
            <td>${escapeHtml(raw.price || raw.price_estimate_min || raw.price_estimate_max || '')}</td>
            <td>${escapeHtml(raw.state || '')}</td>
            <td>${escapeHtml(compactList(raw.images))}</td>
            <td>${escapeHtml(compactList(raw.description))}</td>
            <td>${escapeHtml(page.error || '')}</td>
          </tr>`;
        }).join('')
      }</tbody></table>`;
    }
    function rerenderWorkbench() {
      renderDetail(selectedDetail, selectedMatrix);
    }
    async function reloadMatrix() {
      selectedMatrix = await api(`/api/reviews/${selectedId}/matrix`);
      rerenderWorkbench();
    }
    function schemasPayload() {
      return JSON.parse(document.getElementById('candidatePayload').value);
    }
    function currentSelector() {
      const payload = selectedDetail.review.candidate_payload;
      return (((payload.schemas || [])[selectedSchemaIndex] || {})[selectedField] || {}).selector || '';
    }
    function setCurrentSelector(selector) {
      const payload = schemasPayload();
      payload.schemas = payload.schemas || [];
      payload.schemas[selectedSchemaIndex] = payload.schemas[selectedSchemaIndex] || {};
      payload.schemas[selectedSchemaIndex][selectedField] = payload.schemas[selectedSchemaIndex][selectedField] || defaultRuleFor(selectedField);
      payload.schemas[selectedSchemaIndex][selectedField].selector = selector;
      document.getElementById('candidatePayload').value = JSON.stringify(payload, null, 2);
      selectedDetail.review.candidate_payload = payload;
    }
    function defaultRuleFor(field) {
      if (field === 'images') return { selector: '', additional_selectors: [], type: 'attribute', name: 'src', cardinality: 'all' };
      return { selector: '', additional_selectors: [], type: 'text', cardinality: 'first' };
    }
    let pickedSelector = '';
    function applyPickedSelector() {
      if (!pickedSelector) return;
      setCurrentSelector(pickedSelector);
      document.getElementById('selectedSelector').value = pickedSelector;
      postHighlight(pickedSelector);
    }
    function postHighlight(selector) {
      const frame = document.getElementById('snapshotFrame');
      if (frame && frame.contentWindow) frame.contentWindow.postMessage({ type: 'crawler-review-highlight-selector', selector }, '*');
    }
    function postHighlightSoon() {
      setTimeout(() => postHighlight(currentSelector()), 350);
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
      selectedDetail = await api(`/api/reviews/${selectedId}`);
      selectedMatrix = selectedDetail.review.artifact_type === 'PRODUCT_SCHEMA'
        ? await api(`/api/reviews/${selectedId}/matrix`)
        : null;
      renderDetail(selectedDetail, selectedMatrix);
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
    function escapeHtmlAttr(s) {
      return escapeHtml(s).replace(/`/g, '&#96;');
    }
    function compactList(values) {
      if (!Array.isArray(values) || values.length === 0) return '';
      const preview = values.slice(0, 2).join(' | ');
      return values.length > 2 ? `${preview} (+${values.length - 2})` : preview;
    }
    window.addEventListener('message', event => {
      const data = event.data || {};
      if (data.type !== 'crawler-review-selector-picked') return;
      pickedSelector = data.selector || '';
      const sample = data.text || data.src || data.href || '';
      document.getElementById('pickedElement').innerHTML = `
        <strong>${escapeHtml(pickedSelector)}</strong>
        <div class="muted">${escapeHtml(data.tag || '')} ${escapeHtml(sample)}</div>`;
    });
    loadReviews().catch(err => document.getElementById('reviewList').textContent = err);
  </script>
</body>
</html>"#;
