use super::{ReviewServer, ReviewServerConfig};
use crate::network::policy::NetworkErrorKind;
use crate::review::model::{PAGE_ROLE_PRIMARY, SchemaReviewPageInput};
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml, HtmlFetcher};
use crate::service::crawler_domain_configuration::{
    CrawlerDomainAdministration, CrawlerDomainConfiguration, CrawlerDomainConfigurationError,
    CrawlerDomainRemoval,
};
use crate::test_support::migrated_crawler_database;
use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::{Domain, ListingSourceId};
use serde_json::json;
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
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
        let permit = self
            .released
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
    _admin_pool: PgPool,
    pool: PgPool,
    server: ReviewServer,
    review_id: CrawlerReviewId,
}

const PAGE_COUNT: usize = 2;

async fn fixture(fetcher: Arc<PacedFetcher>) -> Fixture {
    let (admin_pool, pool) = migrated_crawler_database().await;
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
            fetcher,
        )
    };
    Fixture {
        _admin_pool: admin_pool,
        pool,
        server,
        review_id,
    }
}

async fn get_matrix(server: &ReviewServer, review_id: CrawlerReviewId, refresh: bool) -> String {
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

async fn stored_matrix(pool: &PgPool, review_id: CrawlerReviewId) -> Option<serde_json::Value> {
    let summary: serde_json::Value =
        sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap_or_else(|error| panic!("read validation summary: {error}"));
    summary.get("schema_matrix").cloned()
}

#[tokio::test]
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

#[tokio::test]
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

#[tokio::test]
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

#[tokio::test]
async fn should_store_the_matrix_of_the_current_candidate_when_it_changes_during_the_refresh() {
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
