//! `review-mcp` per agent role against a review console of `agent` authoring mode: the console is
//! wired as the crawler wires it, with the agents' tokens provisioned into a token file, and each
//! role's `review-mcp` binary is driven over stdio as an MCP client drives it. Each role lists and
//! runs only its own tools, every rule is the console's (the approval guard refuses, then takes,
//! the reviewer's approval), URLs off the review's ListingSource are refused, and a console restart
//! that rotates the tokens is followed by reading the token file again.

use crawler::CrawlerReviewId;
use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::agent_tokens::{REVIEW_AGENT_TOKEN_FILE, provision_review_agents};
use crawler::review::model::{PAGE_ROLE_PRIMARY, SchemaReviewPageInput, UrlPatternDecision};
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::{ReviewServer, ReviewServerConfig};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
use crawler::spider::classification::url_classifier::UrlClassifier;
use listing_source_core::ListingSourceId;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use test_api::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const SHOP: &str = "https://antiques.example";
const HUMAN_TOKEN: &str = "human-review-token";

/// The shop: `/item/<slug>` is the page of one product with JSON-LD markup, any other path a
/// category page.
struct Shop;

fn product_page(slug: &str) -> String {
    format!(
        r#"<html><head><script type="application/ld+json">{{"@type":"Product","name":"{slug}"}}</script></head><body><span class="id">{slug}</span><h1>{slug}</h1><span class="state">In stock</span><img class="product" src="{slug}.jpg"></body></html>"#
    )
}

#[async_trait::async_trait]
impl HtmlFetcher for Shop {
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
        let html = match url.path().strip_prefix("/item/") {
            Some(slug) => product_page(slug),
            None => "<html><body><h1>Chairs</h1><a href=\"/item/chair\">Chair</a></body></html>"
                .to_owned(),
        };
        Ok(FetchedHtml::new(html, url.clone(), reqwest::StatusCode::OK))
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

fn schema(title_selector: &str) -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id", ExtractionKind::Text)),
        title: Some(rule(title_selector, ExtractionKind::Text)),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: Some(rule("span.state", ExtractionKind::Text)),
        images: Some(ExtractionRule {
            cardinality: ExtractionCardinality::All,
            ..rule(
                "img.product",
                ExtractionKind::Attribute { name: "src".into() },
            )
        }),
        raw_attributes: Default::default(),
    }
}

struct Reviews {
    product_schema: CrawlerReviewId,
    url_pattern: CrawlerReviewId,
}

/// A crawl-enabled ListingSource crawled from the shop's root with a pending product-schema
/// review whose starter schema misses the title, and a pending, undecided URL-pattern review of
/// the source.
async fn reviews(pool: &PgPool) -> Reviews {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, 'Antiques', 'antiques', TRUE, \
                 'https://antiques.example/', 'antiques.example', NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let product_schema = repository
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
        .unwrap();
    let url_pattern = repository
        .create_url_pattern_review(
            &listing_source_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &[
                "/item/chair",
                "/item/table",
                "/item/lamp",
                "/shop/chairs",
                "/about",
                "/impressum",
            ]
            .map(|path| format!("{SHOP}{path}")),
            None,
        )
        .await
        .unwrap();
    Reviews {
        product_schema,
        url_pattern,
    }
}

/// A console of `agent` authoring mode as the crawler starts it, serving on `listener`: its
/// configuration and the agents' limits from the review settings, the agents' tokens provisioned
/// into the token file in `state_directory`, the reviewer agent deciding within the approval guard.
fn start_console(pool: &PgPool, state_directory: &Path, listener: TcpListener) -> JoinHandle<()> {
    let config = ReviewServerConfig::from_env().unwrap();
    let config =
        provision_review_agents(config, CrawlerAuthoringMode::Agent, state_directory).unwrap();
    let agent_decisions = AgentDecisionPolicy::new(
        CrawlerAuthoringMode::Agent,
        AgentLimits::from_env().unwrap(),
    );
    let server = ReviewServer::new(
        CrawlerReviewRepository::new(pool.clone())
            .with_url_classifier(UrlClassifier::for_authoring(CrawlerAuthoringMode::Agent))
            .with_agent_decisions(agent_decisions),
        config,
        Arc::new(Shop),
    );
    tokio::spawn(async move {
        let _ = server.serve(listener).await;
    })
}

/// A `review-mcp` process of one role, driven over stdio.
struct McpSession {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

impl McpSession {
    /// A `review-mcp` session of `role`, scoped to `review` when one is given.
    async fn start(
        role: &str,
        review: Option<CrawlerReviewId>,
        console: SocketAddr,
        token_file: &Path,
    ) -> Self {
        let scope = review.map(|review| format!("--review={review}"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_review-mcp"))
            .args(["--role", role])
            .args(scope)
            .env("CRAWLER_REVIEW_CONSOLE_URL", format!("http://{console}"))
            .env("CRAWLER_REVIEW_AGENT_TOKEN_FILE", token_file)
            .env("LOG_LEVEL", "warn")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut session = Self {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        let initialized = session
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "review-mcp-test", "version": "0" },
                }),
            )
            .await;
        assert_eq!(
            initialized["result"]["serverInfo"]["name"],
            "crawler-review"
        );
        session
            .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        session
    }

    async fn send(&mut self, message: Value) {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        self.stdin.flush().await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(120), self.lines.next_line())
                    .await
                    .expect("review-mcp answers in time")
                    .unwrap()
                    .expect("review-mcp keeps its session open");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(id) {
                return message;
            }
        }
    }

    async fn tool_names(&mut self) -> Vec<String> {
        self.request("tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Calls a tool: its structured answer, or its tool error as `Err`.
    async fn call(&mut self, name: &str, arguments: Value) -> Result<Value, Value> {
        let reply = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await;
        let result = &reply["result"];
        if result.is_null() {
            return Err(reply["error"].clone());
        }
        if result["isError"] == json!(true) {
            let text = result["content"][0]["text"].as_str().unwrap();
            return Err(serde_json::from_str(text).unwrap());
        }
        Ok(result["structuredContent"].clone())
    }

    async fn close(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), self.child.wait())
            .await
            .expect("review-mcp ends with its session")
            .unwrap();
        assert!(status.success(), "{status}");
    }
}

fn set_env(name: &str, value: &str) {
    // SAFETY: this test binary runs this one test, which sets the review settings before it
    // starts any thread that reads the environment.
    unsafe { std::env::set_var(name, value) };
}

/// A state directory of its own, removed when the test ends.
struct StateDirectory(PathBuf);

impl Drop for StateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_work_reviews_through_each_role_s_own_tools_within_the_console_s_rules() {
    set_env("CRAWLER_REVIEW_BIND_ADDR", "127.0.0.1:0");
    set_env("CRAWLER_REVIEW_AUTH_TOKEN", HUMAN_TOKEN);
    let pool = get_postgres_client().await;
    let Reviews {
        product_schema,
        url_pattern,
    } = reviews(&pool).await;
    let state = StateDirectory(std::env::temp_dir().join(format!(
        "crawler-review-mcp-{}",
        uuid::Uuid::new_v4().simple()
    )));
    std::fs::create_dir(&state.0).unwrap();
    let token_file = state.0.join(REVIEW_AGENT_TOKEN_FILE);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let console = listener.local_addr().unwrap();
    let running_console = start_console(&pool, &state.0, listener);

    let mut author = McpSession::start("author", None, console, &token_file).await;
    let mut reviewer = McpSession::start("reviewer", None, console, &token_file).await;

    // Each role lists and runs only its own tools.
    assert_eq!(
        author.tool_names().await,
        [
            "list_pending_reviews",
            "get_review",
            "inspect_page",
            "probe_selectors",
            "refresh_matrix",
            "check_approval",
            "mark_needs_repair",
            "evaluate_candidate",
            "update_candidate",
            "stage_and_check",
            "update_schema_field",
            "add_review_page",
            "remove_review_page",
            "mark_page_not_product",
            "declare_removed_page",
            "record_no_sold_page",
            "label_source",
        ]
    );
    assert_eq!(
        reviewer.tool_names().await,
        [
            "list_pending_reviews",
            "get_review",
            "inspect_page",
            "probe_selectors",
            "refresh_matrix",
            "check_approval",
            "mark_needs_repair",
            "approve",
            "reject",
        ]
    );
    let approve_as_author = author
        .call(
            "approve",
            json!({ "review_id": product_schema.to_string(), "expected_candidate_version": 1 }),
        )
        .await;
    assert_eq!(approve_as_author.unwrap_err()["code"], -32602);
    let edit_as_reviewer = reviewer
        .call(
            "update_schema_field",
            json!({ "review_id": product_schema.to_string() }),
        )
        .await;
    assert_eq!(edit_as_reviewer.unwrap_err()["code"], -32602);

    // A process scoped to one review lists no review queue and refuses every other review; the
    // URL-pattern review's trail at the end shows that the refused reject recorded nothing.
    let mut scoped =
        McpSession::start("reviewer", Some(product_schema), console, &token_file).await;
    assert_eq!(
        scoped.tool_names().await,
        [
            "get_review",
            "inspect_page",
            "probe_selectors",
            "refresh_matrix",
            "check_approval",
            "mark_needs_repair",
            "approve",
            "reject",
        ]
    );
    let own = scoped
        .call(
            "get_review",
            json!({ "review_id": product_schema.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(own["candidate_version"], 1);
    let reject_elsewhere = scoped
        .call(
            "reject",
            json!({ "review_id": url_pattern.to_string(), "notes": "the page says so" }),
        )
        .await
        .unwrap_err();
    assert_eq!(reject_elsewhere["kind"], "REVIEW_OUTSIDE_SCOPE");
    scoped.close().await;

    // Reads: the queue page by page, oldest first.
    let queue = author
        .call("list_pending_reviews", json!({}))
        .await
        .unwrap();
    assert_eq!(
        (
            &queue["total_pending"],
            &queue["waiting_for_human"],
            &queue["more"]
        ),
        (&json!(2), &json!(0), &json!(false))
    );
    assert_eq!(queue["reviews"][0]["review_id"], product_schema.to_string());
    let after_first = author
        .call(
            "list_pending_reviews",
            json!({ "after": product_schema.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(after_first["reviews"], json!([queue["reviews"][1].clone()]));
    assert_eq!(
        after_first["reviews"][0]["review_id"],
        url_pattern.to_string()
    );
    let review = author
        .call(
            "get_review",
            json!({ "review_id": product_schema.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(review["candidate_version"], 1);
    assert_eq!(review["candidate"]["kind"], "PRODUCT_SCHEMA");
    assert_eq!(review["matrix"], Value::Null);
    let page_id = review["pages"][0]["review_page_id"].clone();
    let page = reviewer
        .call(
            "inspect_page",
            json!({ "review_id": product_schema.to_string(), "review_page_id": page_id }),
        )
        .await
        .unwrap();
    assert_eq!(page["describes_one_product"], true);
    assert!(page["projection"].as_str().unwrap().contains("chair"));
    assert_eq!(page["view"], "OUTLINE");
    let markdown = reviewer
        .call(
            "inspect_page",
            json!({ "review_id": product_schema.to_string(), "review_page_id": page_id, "view": "MARKDOWN" }),
        )
        .await
        .unwrap();
    assert_eq!(markdown["view"], "MARKDOWN");
    assert!(markdown["projection"].as_str().unwrap().contains("chair"));
    // A probe answers only what each selector matches, on a review page and a URL of the shop.
    let probed = reviewer
        .call(
            "probe_selectors",
            json!({
                "review_id": product_schema.to_string(),
                "pages": [{ "review_page_id": page_id }, { "url": format!("{SHOP}/item/table") }],
                "selectors": [{ "selector": "h1" }, { "selector": "h1[" }],
            }),
        )
        .await
        .unwrap();
    let probed_pages = probed["pages"].as_array().unwrap();
    assert_eq!(probed_pages.len(), 2);
    assert_eq!(probed_pages[1]["url"], format!("{SHOP}/item/table"));
    for page in probed_pages {
        assert_eq!(page["results"][0]["matches"], 1, "{page}");
        assert!(page["results"][1]["error"].is_string(), "{page}");
    }

    // Every tool that takes a URL refuses one off the review's ListingSource before a request.
    for (tool, arguments) in [
        (
            "inspect_page",
            json!({ "review_id": product_schema.to_string(), "url": "https://elsewhere.example/item/chair" }),
        ),
        (
            "probe_selectors",
            json!({ "review_id": product_schema.to_string(), "pages": [{ "url": "https://elsewhere.example/item/chair" }], "selectors": [{ "selector": "h1" }] }),
        ),
        (
            "add_review_page",
            json!({ "review_id": product_schema.to_string(), "url": "http://127.0.0.1:7878/api/reviews" }),
        ),
        (
            "evaluate_candidate",
            json!({ "review_id": product_schema.to_string(), "schemas": [schema("h1")], "urls": ["https://elsewhere.example/item/table"] }),
        ),
        (
            "record_no_sold_page",
            json!({ "review_id": product_schema.to_string(), "urls": [format!("{SHOP}/item/a"), format!("{SHOP}/item/b"), "https://elsewhere.example/item/c"], "notes": "searched" }),
        ),
    ] {
        let refused = author.call(tool, arguments).await.unwrap_err();
        assert_eq!(
            refused["kind"], "URL_OUTSIDE_REVIEW_SOURCE",
            "{tool}: {refused}"
        );
    }

    // The author tests a candidate on a further page of the shop, then stages it.
    let evaluated = author
        .call(
            "evaluate_candidate",
            json!({
                "review_id": product_schema.to_string(),
                "schemas": [schema("h1")],
                "urls": [format!("{SHOP}/item/table")],
            }),
        )
        .await
        .unwrap();
    let pages = evaluated["matrix"]["schemas"][0]["pages"]
        .as_array()
        .unwrap();
    assert_eq!(pages.len(), 2);
    assert!(pages.iter().all(|page| page["applies"] == json!(true)));
    assert_eq!(pages[1]["page"]["input_index"], 0);
    let staged = author
        .call(
            "update_candidate",
            json!({
                "review_id": product_schema.to_string(),
                "expected_candidate_version": 1,
                "candidate": { "kind": "PRODUCT_SCHEMA", "schemas": [schema("h1")] },
            }),
        )
        .await
        .unwrap();
    assert_eq!(staged["candidate_version"], 2);

    // The author adds a page of the shop that is no product and marks it instead of bending the
    // schema; get_review shows the reason, the marked page is left out of the matrix, and a
    // URL-pattern review takes no mark.
    let added = author
        .call(
            "add_review_page",
            json!({ "review_id": product_schema.to_string(), "url": format!("{SHOP}/item/table") }),
        )
        .await
        .unwrap();
    let marked = author
        .call(
            "mark_page_not_product",
            json!({
                "review_id": product_schema.to_string(),
                "review_page_id": added["review_page_id"],
                "reason": "the dealer's own profile",
            }),
        )
        .await
        .unwrap();
    assert_eq!(marked["reason"], "the dealer's own profile");
    let blank = author
        .call(
            "mark_page_not_product",
            json!({
                "review_id": product_schema.to_string(),
                "review_page_id": added["review_page_id"],
                "reason": " ",
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(blank["kind"], "INVALID_REQUEST", "{blank}");
    let marked_pages = author
        .call(
            "get_review",
            json!({ "review_id": product_schema.to_string() }),
        )
        .await
        .unwrap()["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|page| page["not_product_reason"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        marked_pages,
        [Value::Null, json!("the dealer's own profile")]
    );
    let stale = author
        .call(
            "update_schema_field",
            json!({
                "review_id": product_schema.to_string(),
                "expected_candidate_version": 1,
                "schema_index": 0,
                "field": "title",
                "rule": { "selector": "h1", "type": "text" },
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (stale["kind"].clone(), stale["candidate_version"].clone()),
        (json!("CONFLICT"), json!(2))
    );

    // The shop shows no sold page: the author records the pages it inspected or probed, which the
    // approval guard needs next to the matrix. A page it did neither on is refused.
    let checked =
        ["a", "b", "c", "d", "e", "f", "g", "h"].map(|slug| format!("{SHOP}/item/{slug}"));
    let uninspected = author
        .call(
            "record_no_sold_page",
            json!({
                "review_id": product_schema.to_string(),
                "urls": checked,
                "notes": "searched the listing and its sold section",
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (uninspected["kind"].clone(), uninspected["reason"].clone()),
        (json!("CONFLICT"), json!("PAGES_NOT_INSPECTED"))
    );
    for pages in checked.chunks(4) {
        let pages: Vec<_> = pages.iter().map(|url| json!({ "url": url })).collect();
        author
            .call(
                "probe_selectors",
                json!({ "review_id": product_schema.to_string(), "pages": pages, "selectors": [{ "selector": "span.state" }] }),
            )
            .await
            .unwrap();
    }
    let recorded = author
        .call(
            "record_no_sold_page",
            json!({
                "review_id": product_schema.to_string(),
                "urls": checked,
                "notes": "searched the listing and its sold section",
            }),
        )
        .await
        .unwrap();
    assert_eq!(recorded["recorded_urls"], 8);

    // The reviewer's approval is the console's to take: refused without a live matrix of the
    // staged version, taken once the matrix is refreshed.
    let refused = reviewer
        .call(
            "approve",
            json!({ "review_id": product_schema.to_string(), "expected_candidate_version": 2 }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["kind"], "CONFLICT", "{refused}");
    assert_eq!(
        refused["failed_checks"][0]["check"], "LIVE_MATRIX_MISSING",
        "{refused}"
    );
    let refreshed = reviewer
        .call(
            "refresh_matrix",
            json!({ "review_id": product_schema.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(
        refreshed["matrix"]["live_evaluation"]["candidate_version"],
        2
    );
    let approved = reviewer
        .call("approve", json!({ "review_id": product_schema.to_string(), "expected_candidate_version": 2, "notes": "title and state on the page" }))
        .await
        .unwrap();
    assert_eq!(approved["verdict"], "APPROVED");

    // The author stages a pattern from the sample's URLs alone; one that takes none of them is
    // no usable rule, and the guard's dry run says so before anyone approves.
    let no_match = author
        .call(
            "update_candidate",
            json!({
                "review_id": url_pattern.to_string(),
                "expected_candidate_version": 1,
                "candidate": { "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/produkt/[^/]+$" } },
            }),
        )
        .await
        .unwrap();
    let checked = author
        .call(
            "check_approval",
            json!({ "review_id": url_pattern.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(
        checked["failed_checks"],
        json!([{ "check": "PATTERN_MATCHES_NO_SAMPLE_URL", "sample_total": 6 }]),
        "{checked}"
    );
    let refused = reviewer
        .call("approve", json!({ "review_id": url_pattern.to_string(), "expected_candidate_version": no_match["candidate_version"] }))
        .await
        .unwrap_err();
    assert_eq!(
        refused["failed_checks"][0]["check"], "PATTERN_MATCHES_NO_SAMPLE_URL",
        "{refused}"
    );

    // A console restart rotates the agents' tokens; each server reads its new token on the 401.
    running_console.abort();
    let _ = running_console.await;
    let old_tokens = std::fs::read(&token_file).unwrap();
    let listener = TcpListener::bind(console).await.unwrap();
    let running_console = start_console(&pool, &state.0, listener);
    assert_ne!(std::fs::read(&token_file).unwrap(), old_tokens);

    // A pattern that takes some of the sample's URLs and leaves others is approved without a
    // single sample label.
    let excluded = author
        .call(
            "update_candidate",
            json!({
                "review_id": url_pattern.to_string(),
                "expected_candidate_version": no_match["candidate_version"],
                "candidate": { "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/[^/]+$", "exclude": "-gift-card$" } },
            }),
        )
        .await
        .unwrap();
    let staged = author
        .call(
            "get_review",
            json!({ "review_id": url_pattern.to_string() }),
        )
        .await
        .unwrap();
    assert_eq!(
        staged["candidate"]["decision"],
        json!({
            "type": "PATTERN",
            "value": "/item/[^/]+$",
            "exclude": "-gift-card$",
            "matching": "PATH_AND_QUERY_FULL_MATCH",
        }),
        "{staged}"
    );
    let approved = reviewer
        .call("approve", json!({ "review_id": url_pattern.to_string(), "expected_candidate_version": excluded["candidate_version"] }))
        .await
        .unwrap();
    assert_eq!(approved["verdict"], "APPROVED");
    // An agent's pattern goes live matched against the path and query.
    let live_matching: Option<String> = sqlx::query_scalar(
        "SELECT s.url_pattern_matching FROM listing_sources s \
         JOIN crawler_reviews r ON r.listing_source_id = s.listing_source_id \
         WHERE r.review_id = $1",
    )
    .bind(url_pattern.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_matching.as_deref(), Some("PATH_AND_QUERY_FULL_MATCH"));

    let trail = reviewer
        .call(
            "get_review",
            json!({ "review_id": url_pattern.to_string() }),
        )
        .await
        .unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| (event["actor"].clone(), event["action"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        trail,
        [
            (json!("AGENT_AUTHOR"), json!("CANDIDATE_EDITED")),
            (json!("AGENT_AUTHOR"), json!("CANDIDATE_EDITED")),
            (json!("AGENT_REVIEWER"), json!("APPROVED")),
        ]
    );

    // The author labels the shop of a pending review it works, never through a decided one; the
    // label switches the source's crawling off.
    let decided = author
        .call(
            "label_source",
            json!({ "review_id": url_pattern.to_string(), "label": "NOT_AN_ESHOP", "note": "enquiry only" }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        decided["reason"], "AGENT_CHANGES_ONLY_PENDING_REVIEWS",
        "{decided}"
    );
    let listing_source_id: uuid::Uuid =
        sqlx::query_scalar("SELECT listing_source_id FROM crawler_reviews WHERE review_id = $1")
            .bind(url_pattern.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let pending = CrawlerReviewRepository::new(pool.clone())
        .create_url_pattern_review(
            &ListingSourceId::try_from(listing_source_id).unwrap(),
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &[format!("{SHOP}/item/chair"), format!("{SHOP}/about")],
            None,
        )
        .await
        .unwrap();
    let labelled = author
        .call(
            "label_source",
            json!({
                "review_id": pending.to_string(),
                "label": "NOT_AN_ESHOP",
                "note": "/item/chair shows a price on request and an enquiry form, no cart",
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        (&labelled["review_id"], &labelled["label"]),
        (&json!(pending.to_string()), &json!("NOT_AN_ESHOP")),
        "{labelled}"
    );
    let crawl_enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!crawl_enabled);

    author.close().await;
    reviewer.close().await;
    running_console.abort();
}
