//! A spider crawl that waits on a manual URL-pattern review while an operator decides it, with
//! the crawler's PostgreSQL repositories behind the spider service and the spider scheduler's
//! record of the wait.

use crawler::review::model::{ReviewActor, UrlPatternDecision, UrlPatternReviewCandidate};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository};
use crawler::spider::candidate_service::{
    SPIDER_RECRAWL_INTERVAL, SpiderCandidateService, SpiderCandidateServiceImpl,
    UrlPatternReviewWaitWrite,
};
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::{
    MANUAL_URL_PATTERN_REASON, UrlPatternAuthoring, UrlPatternServiceImpl,
};
use crawler::spider::discovery::website_spider::{
    CrawlCompletion, CrawlDiagnostics, CrawledPage, MockSpider, SpiderCrawl,
};
use crawler::spider::service::{
    SpiderRunResult, SpiderService, SpiderServiceConfig, SpiderServiceError, SpiderServiceImpl,
};
use crawler::spider::utils::url::CrawledUrl;
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use test_api::*;
use time::OffsetDateTime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const DOMAIN: &str = "mid-crawl.example.com";
/// Pages crawled before the spider classifies, enough for the inference sample.
const THRESHOLD: usize = 20;
/// Pages crawled after the operator decided the review.
const PAGES_AFTER_DECISION: usize = 7;

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

/// A ListingSource with one domain whose URL pattern an operator writes (`manual` authoring),
/// with the crawler's PostgreSQL repositories behind the spider service.
struct ManualCrawlFixture {
    pool: PgPool,
    listing_source_id: ListingSourceId,
    domain_id: CrawlerDomainId,
    review_repository: CrawlerReviewRepository,
}

impl ManualCrawlFixture {
    async fn new() -> Self {
        let pool = get_postgres_client().await;
        let listing_source_id = ListingSourceId::new();
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
             VALUES ($1, 'Mid-crawl source', 'mid-crawl-source', TRUE)",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
        let domain_id = CrawlerDomainId::new();
        sqlx::query(
            "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
             VALUES ($1, $2, $3, $3)",
        )
        .bind(domain_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .bind(DOMAIN)
        .execute(&pool)
        .await
        .unwrap();
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        Self {
            pool,
            listing_source_id,
            domain_id,
            review_repository,
        }
    }

    /// Starts a crawl of the domain whose pages the test delivers through the returned sender;
    /// dropping the sender ends the crawl as completed.
    fn start_crawl(
        &self,
    ) -> (
        mpsc::Sender<CrawledPage>,
        JoinHandle<Result<SpiderRunResult, SpiderServiceError>>,
    ) {
        let (pages, spider) = crawl_fed_by_the_test();
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size: 5,
                ..SpiderServiceConfig::default()
            },
            Box::new(spider),
            Box::new(UrlPatternServiceImpl::new(
                Arc::new(ListingSourceUrlPatternRepositoryImpl::new(
                    self.pool.clone(),
                )),
                UrlPatternAuthoring::Manual {
                    review_repository: self.review_repository.clone(),
                },
            )),
            Arc::new(UrlMetadataRepositoryImpl::new(self.pool.clone())),
        );
        let (listing_source_id, domain_id) = (self.listing_source_id, self.domain_id);
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
        (pages, crawl)
    }

    /// Delivers the crawl's pages up to its threshold and returns the manual URL-pattern review
    /// the crawl waits on, once the crawl has stored those pages.
    async fn crawl_until_the_review_waits(
        &self,
        pages: &mpsc::Sender<CrawledPage>,
    ) -> CrawlerReviewId {
        send_pages(pages, 0..THRESHOLD).await;
        let review_id = eventually("the manual URL-pattern review", || async {
            self.review_repository
                .latest_pending_url_pattern_review_id(&self.listing_source_id, &self.domain_id)
                .await
                .unwrap()
        })
        .await;
        eventually("the URLs crawled before the decision", || async {
            (self.stored_url_classes().await.len() == THRESHOLD).then_some(())
        })
        .await;
        review_id
    }

    /// Records the crawl's wait on `review_id` as the spider scheduler does once a crawl that
    /// waited on the review ends.
    async fn record_review_wait(&self, review_id: CrawlerReviewId) -> UrlPatternReviewWaitWrite {
        SpiderCandidateServiceImpl::new(self.pool.clone())
            .mark_crawl_awaiting_url_pattern_review(
                &self.domain_id,
                &review_id,
                OffsetDateTime::now_utc() + SPIDER_RECRAWL_INTERVAL,
            )
            .await
            .unwrap()
    }

    /// Opens the domain's manual URL-pattern review and records the wait on it of an earlier
    /// crawl that ended while the review was pending.
    async fn review_an_earlier_crawl_waits_on(&self) -> CrawlerReviewId {
        let review_id = self
            .review_repository
            .create_url_pattern_review(
                &self.listing_source_id,
                &self.domain_id,
                MANUAL_URL_PATTERN_REASON,
                UrlPatternDecision::Undecided,
                &[format!("https://{DOMAIN}/item/0")],
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            self.record_review_wait(review_id).await,
            UrlPatternReviewWaitWrite::Recorded
        );
        review_id
    }

    /// The domain's recorded crawl outcome and the time it is due again.
    async fn crawl_schedule(&self) -> (Option<String>, Option<OffsetDateTime>) {
        sqlx::query_as(
            "SELECT last_crawl_outcome, next_crawl_at FROM listing_source_domains WHERE domain_id = $1",
        )
        .bind(self.domain_id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn stored_url_classes(&self) -> Vec<(String, String)> {
        sqlx::query_as(
            "SELECT url, url_class FROM listing_source_urls WHERE domain_id = $1 ORDER BY url",
        )
        .bind(self.domain_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

/// Writes `pattern` as the review's decision, as an operator does in the console.
async fn stage_pattern(
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
}

/// Approves the review's current candidate, as an operator does in the console.
async fn approve_candidate(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) {
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
    let fixture = ManualCrawlFixture::new().await;
    let (pages, crawl) = fixture.start_crawl();

    let review_id = fixture.crawl_until_the_review_waits(&pages).await;
    stage_pattern(&fixture.review_repository, review_id, "/item/").await;
    approve_candidate(&fixture.review_repository, review_id).await;
    send_pages(&pages, THRESHOLD..THRESHOLD + PAGES_AFTER_DECISION).await;
    drop(pages);
    let run = crawl
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("crawl: {error}"));

    let crawled = THRESHOLD + PAGES_AFTER_DECISION;
    let item_pages = crawled.div_ceil(2);
    assert_eq!(
        run.pending_url_pattern_review, None,
        "the crawl no longer waits on the approved review"
    );
    assert_eq!(run.completion, CrawlCompletion::Completed);
    assert_eq!(run.product_pattern.as_deref(), Some("/item/"));
    assert_eq!(run.product_urls_count, item_pages);
    let stored = fixture.stored_url_classes().await;
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

/// The crawl opens a manual URL-pattern review at its threshold and stores its URLs without a
/// pattern; the operator rejects the review while it still crawls. A rejection stores no
/// decision, so the crawl ends still waiting on the review with its URLs unclassified, but its
/// wait is not recorded: the domain keeps the crawl state the rejection left, without the
/// review-wait outcome and the timer that would hold it back.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_record_no_review_wait_when_url_pattern_review_is_rejected_during_the_crawl() {
    let fixture = ManualCrawlFixture::new().await;
    let (pages, crawl) = fixture.start_crawl();

    let review_id = fixture.crawl_until_the_review_waits(&pages).await;
    fixture
        .review_repository
        .reject_review(
            review_id,
            ReviewActor::Human,
            Some("not the shop's catalogue"),
        )
        .await
        .unwrap();
    let schedule_after_rejection = fixture.crawl_schedule().await;
    send_pages(&pages, THRESHOLD..THRESHOLD + PAGES_AFTER_DECISION).await;
    drop(pages);
    let run = crawl
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("crawl: {error}"));
    let wait_write = fixture.record_review_wait(review_id).await;

    assert_eq!(run.pending_url_pattern_review, Some(review_id));
    let stored = fixture.stored_url_classes().await;
    assert_eq!(stored.len(), THRESHOLD + PAGES_AFTER_DECISION);
    assert!(
        stored.iter().all(|(_, class)| class != "product"),
        "a rejection classifies no URL: {stored:?}"
    );
    assert_eq!(wait_write, UrlPatternReviewWaitWrite::ReviewNotPending);
    let schedule = fixture.crawl_schedule().await;
    assert_eq!(
        schedule, schedule_after_rejection,
        "the domain keeps the crawl state the rejection left"
    );
    assert_eq!(
        schedule,
        (None, None),
        "no review-wait outcome or timer holds the domain back"
    );
}

/// An earlier crawl left the domain waiting on its manual URL-pattern review, and the domain is
/// crawled again while the review is still pending, as it is once that wait has run out. The
/// crawl waits on the same review, and the operator rejects it while the crawl still runs. The
/// rejection releases the domain and clears the earlier crawl's review-wait outcome, and the
/// crawl's own wait is not recorded, so the domain reports no wait on a review it no longer has.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_clear_an_earlier_review_wait_when_its_review_is_rejected_during_a_later_crawl() {
    let fixture = ManualCrawlFixture::new().await;
    let review_id = fixture.review_an_earlier_crawl_waits_on().await;
    let (pages, crawl) = fixture.start_crawl();

    assert_eq!(
        fixture.crawl_until_the_review_waits(&pages).await,
        review_id
    );
    fixture
        .review_repository
        .reject_review(
            review_id,
            ReviewActor::Human,
            Some("not the shop's catalogue"),
        )
        .await
        .unwrap();
    let schedule_after_rejection = fixture.crawl_schedule().await;
    drop(pages);
    let run = crawl
        .await
        .unwrap()
        .unwrap_or_else(|error| panic!("crawl: {error}"));
    let wait_write = fixture.record_review_wait(review_id).await;

    assert_eq!(run.pending_url_pattern_review, Some(review_id));
    assert_eq!(wait_write, UrlPatternReviewWaitWrite::ReviewNotPending);
    assert_eq!(
        schedule_after_rejection,
        (None, None),
        "the rejection released the domain and cleared the earlier crawl's review wait"
    );
    assert_eq!(
        fixture.crawl_schedule().await,
        (None, None),
        "the crawl that waited on the rejected review records no wait"
    );
}

/// A verdict that closes the URL-pattern review a crawl waits on.
#[derive(Clone, Copy, Debug)]
enum ClosingVerdict {
    Approve,
    Reject,
}

impl ClosingVerdict {
    async fn close(self, repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) {
        match self {
            Self::Approve => approve_candidate(repository, review_id).await,
            Self::Reject => repository
                .reject_review(review_id, ReviewActor::Human, None)
                .await
                .unwrap(),
        }
    }
}

/// A crawl that records its wait on a URL-pattern review while a verdict on the review commits,
/// such as an approval just after the crawl's last re-read of the domain's pattern, must not
/// hold back the domain the verdict releases: the verdict releases the domain in the transaction
/// that closes the review, so the wait write waits for it and then finds the review closed.
async fn assert_wait_written_while_the_verdict_commits_keeps_the_release(verdict: ClosingVerdict) {
    let fixture = Arc::new(ManualCrawlFixture::new().await);
    // An earlier crawl waits on the review, so the verdict has a domain to release.
    let review_id = fixture.review_an_earlier_crawl_waits_on().await;
    if let ClosingVerdict::Approve = verdict {
        stage_pattern(&fixture.review_repository, review_id, "/item/").await;
    }
    // Holding the domain's row stops the verdict at the domain, after it has locked the review
    // but before it commits.
    let mut domain_lock = fixture.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM listing_source_domains WHERE domain_id = $1 FOR UPDATE")
        .bind(fixture.domain_id.as_uuid())
        .execute(&mut *domain_lock)
        .await
        .unwrap();
    let closing = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { verdict.close(&fixture.review_repository, review_id).await }
    });
    wait_for_domain_lock_waiters(&fixture.pool, 1).await;
    assert_eq!(
        fixture
            .review_repository
            .latest_pending_url_pattern_review_id(&fixture.listing_source_id, &fixture.domain_id)
            .await
            .unwrap(),
        Some(review_id),
        "{verdict:?} must reach the domain before it commits the closed review"
    );
    // The crawl's last re-read found the review pending, and it records its wait now.
    let wait_write = tokio::spawn({
        let fixture = Arc::clone(&fixture);
        async move { fixture.record_review_wait(review_id).await }
    });
    wait_for_domain_lock_waiters(&fixture.pool, 2).await;

    domain_lock.commit().await.unwrap();

    closing.await.unwrap();
    assert_eq!(
        wait_write.await.unwrap(),
        UrlPatternReviewWaitWrite::ReviewNotPending
    );
    assert_eq!(
        fixture.crawl_schedule().await,
        (None, None),
        "{verdict:?} released the domain and cleared its review wait, and the crawl's wait must \
         not hold it back or report it again"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_release_when_the_review_wait_is_written_while_the_approval_commits() {
    assert_wait_written_while_the_verdict_commits_keeps_the_release(ClosingVerdict::Approve).await;
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_release_when_the_review_wait_is_written_while_the_rejection_commits() {
    assert_wait_written_while_the_verdict_commits_keeps_the_release(ClosingVerdict::Reject).await;
}

/// Waits until `count` statements on a domain row are blocked on a lock.
async fn wait_for_domain_lock_waiters(pool: &PgPool, count: i64) {
    eventually(
        &format!("{count} domain statements waiting on a lock"),
        || async {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pg_stat_activity \
                 WHERE datname = current_database() \
                   AND wait_event_type = 'Lock' \
                   AND query LIKE '%listing_source_domains%'",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            (waiting >= count).then_some(())
        },
    )
    .await;
}
