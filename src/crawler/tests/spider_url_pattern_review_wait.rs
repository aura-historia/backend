//! A spider crawl that waits on a manual URL-pattern review while an operator approves it, with
//! the crawler's PostgreSQL repositories behind the spider service.

use crawler::review::model::{ReviewActor, UrlPatternDecision, UrlPatternReviewCandidate};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository};
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::{
    UrlPatternAuthoring, UrlPatternServiceImpl,
};
use crawler::spider::discovery::website_spider::{
    CrawlCompletion, CrawlDiagnostics, CrawledPage, MockSpider, SpiderCrawl,
};
use crawler::spider::service::{SpiderService, SpiderServiceConfig, SpiderServiceImpl};
use crawler::spider::utils::url::CrawledUrl;
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use test_api::*;
use tokio::sync::{mpsc, oneshot};

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const DOMAIN: &str = "mid-crawl.example.com";
/// Pages crawled before the spider classifies, enough for the inference sample.
const THRESHOLD: usize = 20;
/// Pages crawled after the operator approved the review.
const PAGES_AFTER_APPROVAL: usize = 7;

async fn insert_listing_source(pool: &PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
         VALUES ($1, 'Mid-crawl source', 'mid-crawl-source', TRUE)",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(pool: &PgPool, listing_source_id: ListingSourceId) -> CrawlerDomainId {
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $3)",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(DOMAIN)
    .execute(pool)
    .await
    .unwrap();
    domain_id
}

/// A crawl whose pages the test delivers through the returned sender; dropping it ends the
/// crawl as completed.
fn crawl_fed_by_the_test() -> (mpsc::Sender<CrawledPage>, MockSpider) {
    let (pages_tx, pages_rx) = mpsc::channel(64);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
    let mut spider = MockSpider::new();
    spider.expect_crawl().return_once(move |_| {
        Box::pin(async move {
            Ok(SpiderCrawl {
                pages: pages_rx,
                diagnostics: diagnostics_rx,
            })
        })
    });
    (pages_tx, spider)
}

/// The shop's page `index`: every other page is an item page.
fn shop_page(index: usize) -> CrawledPage {
    let path = if index.is_multiple_of(2) {
        format!("/item/{index}")
    } else {
        format!("/about/{index}")
    };
    CrawledPage {
        url: CrawledUrl::new(url::Url::parse(&format!("https://{DOMAIN}{path}")).unwrap()),
    }
}

async fn send_pages(pages: &mpsc::Sender<CrawledPage>, indexes: std::ops::Range<usize>) {
    for index in indexes {
        pages.send(shop_page(index)).await.unwrap();
    }
}

/// Polls `probe` until it yields a value, failing the test after ten seconds.
async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn stored_url_classes(pool: &PgPool, domain_id: CrawlerDomainId) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT url, url_class FROM listing_source_urls WHERE domain_id = $1 ORDER BY url",
    )
    .bind(domain_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Writes `pattern` as the review's decision and approves it, as an operator does in the console.
async fn approve_pattern(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    pattern: &str,
) {
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            repository.candidate_version(review_id).await.unwrap(),
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: pattern.to_owned(),
                },
                current_pattern: None,
                sample_labels: None,
            }),
        )
        .await
        .unwrap();
    repository
        .approve_review(
            review_id,
            ReviewActor::Human,
            Approval {
                expected_candidate_version: repository.candidate_version(review_id).await.unwrap(),
                notes: None,
                incomplete_coverage: IncompleteCoverage::Refuse,
            },
        )
        .await
        .unwrap();
}

/// The crawl opens a manual URL-pattern review at its threshold and stores its URLs without a
/// pattern; the operator approves a pattern while it still crawls. Every URL of the crawl, those
/// it stores after the approval among them, ends up classified with the approved pattern, and
/// the crawl completes instead of waiting on the review.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_classify_urls_and_complete_when_url_pattern_review_is_approved_during_the_crawl() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id).await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let (pages, spider) = crawl_fed_by_the_test();
    let service = SpiderServiceImpl::new(
        SpiderServiceConfig {
            db_batch_size: 5,
            ..SpiderServiceConfig::default()
        },
        Box::new(spider),
        Box::new(UrlPatternServiceImpl::new(
            Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
            UrlPatternAuthoring::Manual {
                review_repository: review_repository.clone(),
            },
        )),
        Arc::new(UrlMetadataRepositoryImpl::new(pool.clone())),
    );
    let crawl = tokio::spawn(async move {
        service
            .run(
                &listing_source_id,
                &domain_id,
                &format!("https://{DOMAIN}"),
                THRESHOLD,
            )
            .await
    });

    send_pages(&pages, 0..THRESHOLD).await;
    let review_id = eventually("the manual URL-pattern review", || async {
        review_repository
            .latest_pending_url_pattern_review_id(&listing_source_id, &domain_id)
            .await
            .unwrap()
    })
    .await;
    eventually("the URLs crawled before the approval", || async {
        (stored_url_classes(&pool, domain_id).await.len() == THRESHOLD).then_some(())
    })
    .await;
    approve_pattern(&review_repository, review_id, "/item/").await;
    send_pages(&pages, THRESHOLD..THRESHOLD + PAGES_AFTER_APPROVAL).await;
    drop(pages);
    let run = crawl
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("crawl: {error}"));

    let crawled = THRESHOLD + PAGES_AFTER_APPROVAL;
    let item_pages = crawled.div_ceil(2);
    assert_eq!(
        run.pending_url_pattern_review, None,
        "the crawl no longer waits on the approved review"
    );
    assert_eq!(run.completion, CrawlCompletion::Completed);
    assert_eq!(run.product_pattern.as_deref(), Some("/item/"));
    assert_eq!(run.product_urls_count, item_pages);
    let stored = stored_url_classes(&pool, domain_id).await;
    assert_eq!(stored.len(), crawled);
    let misclassified: Vec<_> = stored
        .iter()
        .filter(|(url, class)| (class == "product") != url.contains("/item/"))
        .collect();
    assert!(
        misclassified.is_empty(),
        "every crawled URL is classified with the approved pattern: {misclassified:?}"
    );
}
