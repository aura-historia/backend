use crawler::CrawlerReviewId;
use crawler::network::policy::DomainFailureKind;
use crawler::scraper::candidate_service::{
    FetchFailureRequest, RedirectDestination, RedirectUrlMove, SchemaReviewWait,
    SchemaReviewWaitRequest, ScrapeCircuitOpenRequest, ScrapeCompletion, ScrapeHealthSnapshot,
    ScraperCandidateService, ScraperCandidateServiceImpl, SourceAwaitingReview, UnsettledRedirect,
    UnsettledRedirectRequest,
};
use crawler::spider::candidate_service::{
    AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME, CrawlRateLimitWrite, NEEDS_OPERATOR_CRAWL_OUTCOME,
    RATE_LIMITED_CRAWL_OUTCOME, SpiderCandidateService, SpiderCandidateServiceImpl,
    UrlPatternReviewWaitWrite,
};
use crawler::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use crawler::spider::crawl_frontier_repository::{
    CrawlFrontierRepository, CrawlFrontierRepositoryImpl, KnownUrls,
};
use crawler::spider::crawled_root::CrawledRoot;
use crawler::spider::discovery::frontier::CrawlKind;
use crawler::spider::discovery::website_spider::CrawlCompletion;
use listing_source_core::ListingSourceId;
use std::collections::HashMap;
use std::sync::Arc;
use test_api::*;
use tokio::sync::Barrier;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn raw_input_hash() -> Vec<u8> {
    vec![7; 32]
}

fn scrape_completion<H, S, R, I>(
    listing_source_id: ListingSourceId,
    url: url::Url,
    (
        hash,
        schema_fingerprint,
        raw_input_sha256,
        source_listing_id,
        disposition,
        expected_last_captured_raw_input_sha256,
    ): (H, S, R, I, CrawlerDisposition, Option<Vec<u8>>),
) -> ScrapeCompletion
where
    H: Into<String>,
    S: Into<String>,
    R: Into<Vec<u8>>,
    I: Into<String>,
{
    ScrapeCompletion {
        listing_source_id,
        url,
        hash: hash.into(),
        schema_fingerprint: schema_fingerprint.into(),
        raw_input_sha256: raw_input_sha256.into(),
        source_listing_id: source_listing_id.into(),
        title: "Captured title".to_owned(),
        disposition,
        expected_last_captured_raw_input_sha256,
    }
}

fn fetch_failure_request(
    listing_source_id: ListingSourceId,
    url: url::Url,
    error_kind: &str,
    error_message: &str,
    status_code: Option<i32>,
    next_retry_at: time::OffsetDateTime,
    expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
) -> FetchFailureRequest {
    FetchFailureRequest {
        listing_source_id,
        url,
        error_kind: error_kind.to_owned(),
        error_message: error_message.to_owned(),
        status_code,
        next_retry_at,
        expected_last_captured_raw_input_sha256,
    }
}

fn circuit_open_request(
    url_failure: FetchFailureRequest,
    expected_scrape_health: ScrapeHealthSnapshot,
    scrape_error_kind: DomainFailureKind,
    scrape_status_code: Option<i32>,
    next_scrape_at: time::OffsetDateTime,
) -> ScrapeCircuitOpenRequest {
    ScrapeCircuitOpenRequest {
        url_failure,
        expected_scrape_health,
        scrape_error_kind,
        scrape_status_code,
        next_scrape_at: Some(next_scrape_at),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Inserts a crawl-enabled ListingSource crawled from `https://{crawl_host}/`.
async fn insert_listing_source(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    crawl_host: &str,
) {
    sqlx::query(
        "INSERT INTO listing_sources \
             (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
              crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, $2, $3, TRUE, $4, $5, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind("Test source")
    .bind("test-source")
    .bind(format!("https://{crawl_host}/"))
    .bind(crawl_host)
    .execute(pool)
    .await
    .unwrap();
}

/// The crawl root a spider run of a source [`insert_listing_source`] inserted crawls.
fn crawled_root(listing_source_id: ListingSourceId, crawl_host: &str) -> CrawledRoot {
    CrawledRoot::new(listing_source_id, format!("https://{crawl_host}/"))
}

// ============================================================================
// SPIDER CANDIDATE SERVICE
// ============================================================================

// ---------------------------------------------------------------------------
// get_candidates — a newly-inserted ListingSource (last_crawled IS NULL) is returned
//                  and carries its crawl root
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_return_candidate_with_its_crawl_root_when_never_crawled() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-never-crawled.example.com").await;

    let candidates = service.get_candidates(10, &[]).await.unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].listing_source_id, listing_source_id);
    assert_eq!(
        candidates[0].crawl_root.url().as_str(),
        "https://spider-never-crawled.example.com/",
        "candidate must carry the crawl root stored on its ListingSource"
    );
    assert_eq!(
        candidates[0].crawl_root.host(),
        "spider-never-crawled.example.com"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — a ListingSource crawled recently is NOT returned
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_not_return_candidate_when_recently_crawled() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-recent.example.com").await;

    sqlx::query(
        "UPDATE listing_sources SET last_crawled = NOW(), last_crawl_run_at = NOW() \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(10, &[]).await.unwrap();

    assert!(
        !candidates
            .iter()
            .any(|c| c.listing_source_id == listing_source_id),
        "a ListingSource crawled just now should not be returned as a candidate"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — a recent full crawl makes the ListingSource due for an incremental crawl a
//                  day after its last run; an old one for a full crawl
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_select_full_and_incremental_crawls_by_their_intervals() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let incremental = ListingSourceId::new();
    insert_listing_source(&pool, incremental, "spider-daily.example.com").await;
    let full = ListingSourceId::new();
    insert_listing_source(&pool, full, "spider-weekly.example.com").await;
    let ran_today = ListingSourceId::new();
    insert_listing_source(&pool, ran_today, "spider-today.example.com").await;
    let held = ListingSourceId::new();
    insert_listing_source(&pool, held, "spider-rate-limit-held.example.com").await;
    let set_timers =
        |listing_source_id: ListingSourceId, last_crawled: &'static str, last_run: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "UPDATE listing_sources \
                 SET last_crawled = NOW() - $2::text::interval, \
                     last_crawl_run_at = NOW() - $3::text::interval \
                 WHERE listing_source_id = $1",
                )
                .bind(listing_source_id.as_uuid())
                .bind(last_crawled)
                .bind(last_run)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
    set_timers(incremental, "3 days", "25 hours").await;
    set_timers(full, "8 days", "25 hours").await;
    set_timers(ran_today, "3 days", "2 hours").await;
    set_timers(held, "3 days", "25 hours").await;
    sqlx::query(
        "UPDATE listing_sources SET crawl_rate_limited_until = NOW() + INTERVAL '1 hour' \
         WHERE listing_source_id = $1",
    )
    .bind(held.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let candidates: HashMap<ListingSourceId, CrawlKind> = service
        .get_candidates(10, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|candidate| (candidate.listing_source_id, candidate.kind))
        .collect();

    assert_eq!(candidates.get(&incremental), Some(&CrawlKind::Incremental));
    assert_eq!(candidates.get(&full), Some(&CrawlKind::Full));
    assert_eq!(candidates.get(&ran_today), None);
    assert_eq!(
        candidates.get(&held),
        None,
        "an incremental crawl must not bypass a rate-limit hold"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_stopped_should_keep_failure_and_truncation_state_and_come_due_soon() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-stopped.example.com").await;
    let root = crawled_root(listing_source_id, "spider-stopped.example.com");
    sqlx::query(
        "UPDATE listing_sources \
         SET crawl_truncation_count = 2, crawl_failure_count = 1, \
             last_crawl_error_kind = 'ConnectError' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    service
        .mark_crawl_stopped(
            &root,
            time::OffsetDateTime::now_utc() - time::Duration::seconds(1),
        )
        .await
        .unwrap();

    let (failures, error_kind, next_crawl_at, outcome, truncations, last_crawled) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!((failures, error_kind.as_deref()), (1, Some("ConnectError")));
    assert!(next_crawl_at.is_some());
    assert_eq!(outcome.as_deref(), Some("STOPPED"));
    assert_eq!(truncations, 2, "a stop is not a truncation");
    assert_eq!(last_crawled, None, "a stopped crawl is no full crawl");
    let due: Vec<ListingSourceId> = service
        .get_candidates(1000, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.listing_source_id)
        .collect();
    assert!(
        due.contains(&listing_source_id),
        "the stopped crawl is due again"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_incremental_crawl_finished_should_keep_full_crawl_and_rate_limit_state() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-incremental.example.com").await;
    let root = crawled_root(listing_source_id, "spider-incremental.example.com");
    sqlx::query(
        "UPDATE listing_sources \
         SET last_crawled = NOW() - INTERVAL '3 days', crawl_truncation_count = 2, \
             crawl_rate_limit_streak = 1, crawl_failure_count = 1, \
             last_crawl_error_kind = 'ConnectError' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    service
        .mark_incremental_crawl_finished(&root, CrawlCompletion::HardTimeoutTruncated)
        .await
        .unwrap();
    let (failures, error_kind, next_crawl_at, outcome, truncations, last_crawled) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!((failures, error_kind.as_deref()), (1, Some("ConnectError")));
    assert_eq!(next_crawl_at, None);
    assert_eq!(outcome.as_deref(), Some("HARD_TIMEOUT_TRUNCATED"));
    assert_eq!(truncations, 2);

    service
        .mark_incremental_crawl_finished(&root, CrawlCompletion::Completed)
        .await
        .unwrap();
    let (failures, error_kind, _, outcome, truncations, last_crawled_after) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!((failures, error_kind), (0, None));
    assert_eq!(outcome.as_deref(), Some("COMPLETED"));
    assert_eq!(truncations, 2);
    assert_eq!(last_crawled_after, last_crawled, "never a full crawl");
    let (streak, last_run): (i32, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT crawl_rate_limit_streak, last_crawl_run_at FROM listing_sources \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(streak, 1, "only a full crawl clears the rate-limit streak");
    assert!(last_run.is_some());
    assert!(service.get_candidates(10, &[]).await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// crawl frontier — replaced per crawl, read back in full, kept per ListingSource
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_crawl_frontier_should_be_replaced_by_each_crawl_and_known_products_listed() {
    let pool = get_postgres_client().await;
    let repository = CrawlFrontierRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-frontier.example.com").await;
    let root = crawled_root(listing_source_id, "spider-frontier.example.com");
    let other = ListingSourceId::new();
    insert_listing_source(&pool, other, "spider-other.example.com").await;
    let other_root = crawled_root(other, "spider-other.example.com");
    let url = |path: &str| {
        url::Url::parse(&format!("https://spider-frontier.example.com{path}")).unwrap()
    };

    repository
        .replace_frontier(&root, &[url("/a"), url("/b"), url("/a")])
        .await
        .unwrap();
    repository
        .replace_frontier(&other_root, &[url("/other")])
        .await
        .unwrap();
    assert_eq!(
        repository.load_frontier(&listing_source_id).await.unwrap(),
        [url("/a"), url("/b")]
    );

    repository
        .replace_frontier(&root, &[url("/c")])
        .await
        .unwrap();
    assert_eq!(
        repository.load_frontier(&listing_source_id).await.unwrap(),
        [url("/c")]
    );
    repository.replace_frontier(&root, &[]).await.unwrap();
    assert!(
        repository
            .load_frontier(&listing_source_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repository.load_frontier(&other).await.unwrap(),
        [url("/other")]
    );

    insert_product_url(
        &pool,
        listing_source_id,
        "https://spider-frontier.example.com/product/1",
    )
    .await;
    UrlMetadataRepositoryImpl::new(pool.clone())
        .upsert_link(&listing_source_id, &url("/about"), &UrlClass::Other)
        .await
        .unwrap();
    insert_product_url(
        &pool,
        listing_source_id,
        "https://spider-frontier.example.com/collections/all/products/1",
    )
    .await;
    sqlx::query("UPDATE listing_source_urls SET crawler_disposition = 'DUPLICATE' WHERE url = $1")
        .bind(url("/collections/all/products/1").as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repository.known_urls(&listing_source_id).await.unwrap(),
        KnownUrls {
            products: vec![url("/product/1")],
            retired: vec![url("/collections/all/products/1")],
        },
        "a URL retired as a duplicate is known too, but not as a product URL"
    );

    insert_product_url(
        &pool,
        listing_source_id,
        "https://spider-frontier.example.com/collections/all/products/2/",
    )
    .await;
    sqlx::query("UPDATE listing_source_urls SET crawler_disposition = 'DUPLICATE' WHERE url = $1")
        .bind(url("/collections/all/products/2/").as_str())
        .execute(&pool)
        .await
        .unwrap();

    repository
        .replace_frontier(
            &root,
            &[
                url("/collections/all/products/1"),
                url("/collections/all/products/1/"),
                url("/collections/all/products/2"),
                url("/d"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        repository.load_frontier(&listing_source_id).await.unwrap(),
        [url("/d")],
        "the frontier never carries a URL retired as a duplicate, in either spelling"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — limit is respected
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_respect_limit_when_multiple_candidates_exist() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    for i in 0..3u32 {
        let listing_source_id = ListingSourceId::new();
        insert_listing_source(
            &pool,
            listing_source_id,
            &format!("spider-limit-{}.example.com", i),
        )
        .await;
    }

    let candidates = service.get_candidates(2, &[]).await.unwrap();

    assert_eq!(
        candidates.len(),
        2,
        "get_candidates(2) should return at most 2 results"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — listing_source_id on the candidate matches the owning ListingSource
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_return_correct_listing_source_id_on_candidate() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-source-id.example.com").await;

    let candidates = service.get_candidates(10, &[]).await.unwrap();

    let candidate = candidates
        .iter()
        .find(|c| c.listing_source_id == listing_source_id)
        .expect("candidate for the inserted ListingSource should be present");

    assert_eq!(
        candidate.listing_source_id, listing_source_id,
        "candidate.listing_source_id must match the owning ListingSource"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — failure metadata is included for retry policy decisions
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_return_crawl_failure_metadata_on_candidate() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-failure-meta.example.com").await;

    sqlx::query(
        "UPDATE listing_sources
         SET crawl_failure_count = 2,
             last_crawl_error_kind = 'InsufficientInferenceSample'
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(10, &[]).await.unwrap();
    let candidate = candidates
        .iter()
        .find(|c| c.listing_source_id == listing_source_id)
        .expect("candidate for the inserted ListingSource should be present");

    assert_eq!(candidate.crawl_failure_count, 2);
    assert_eq!(
        candidate.last_crawl_error_kind.as_deref(),
        Some("InsufficientInferenceSample")
    );
}

// ---------------------------------------------------------------------------
// mark_crawl_failure — stores the caller-provided consecutive count and retry
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_failure_should_store_count_kind_and_next_crawl_at() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-mark-failure.example.com").await;
    let root = crawled_root(listing_source_id, "spider-mark-failure.example.com");
    let next_crawl_at = time::OffsetDateTime::now_utc() + time::Duration::hours(6);

    service
        .mark_crawl_failure(&root, "InsufficientInferenceSample", 3, None, next_crawl_at)
        .await
        .unwrap();

    let row: (i32, Option<String>, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT crawl_failure_count, last_crawl_error_kind, next_crawl_at
         FROM listing_sources
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, 3);
    assert_eq!(row.1.as_deref(), Some("InsufficientInferenceSample"));
    let stored_next_crawl_at = row.2.expect("next_crawl_at should be stored");
    assert!(
        (stored_next_crawl_at - next_crawl_at).whole_seconds().abs() <= 1,
        "next_crawl_at should match the caller-provided timestamp"
    );
}

type CrawlStateRow = (
    i32,
    Option<String>,
    Option<time::OffsetDateTime>,
    Option<String>,
    i32,
    Option<time::OffsetDateTime>,
);

async fn crawl_state(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) -> CrawlStateRow {
    sqlx::query_as(
        "SELECT crawl_failure_count, last_crawl_error_kind, next_crawl_at,
                last_crawl_outcome, crawl_truncation_count, last_crawled
         FROM listing_sources
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// crawl outcomes — truncation keeps failure state, completion clears it
// ---------------------------------------------------------------------------

async fn due_spider_candidate(
    pool: &sqlx::PgPool,
    service: &SpiderCandidateServiceImpl,
    listing_source_id: ListingSourceId,
) -> Option<crawler::spider::candidate_service::SpiderCandidate> {
    sqlx::query(
        "UPDATE listing_sources SET next_crawl_at = NOW() - INTERVAL '1 minute' WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    service
        .get_candidates(10, &[])
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.listing_source_id == listing_source_id)
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_rate_limit_state_should_slow_the_source_until_a_full_crawl() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-rate-limited.example.com").await;
    let root = crawled_root(listing_source_id, "spider-rate-limited.example.com");
    service
        .mark_crawl_failure(
            &root,
            "ConnectError",
            1,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    let not_before = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    service
        .mark_crawl_rate_limited(
            &root,
            CrawlRateLimitWrite {
                streak: 2,
                not_before,
            },
        )
        .await
        .unwrap();

    let (failure_count, error_kind, stored_next, outcome, truncations, last_crawled) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!(outcome.as_deref(), Some(RATE_LIMITED_CRAWL_OUTCOME));
    assert_eq!(
        (failure_count, error_kind.as_deref()),
        (1, Some("ConnectError"))
    );
    assert_eq!(
        truncations, 0,
        "a rate-limited crawl is no hard-timeout truncation"
    );
    assert!(
        last_crawled.is_none(),
        "a rate-limited crawl is not a full crawl"
    );
    assert!((stored_next.unwrap() - not_before).whole_seconds().abs() <= 1);

    service
        .mark_crawl_truncated(&root, 1, time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    assert!(
        due_spider_candidate(&pool, &service, listing_source_id)
            .await
            .is_none(),
        "no other crawl-timer write brings the next crawl before the shop's limit"
    );
    sqlx::query(
        "UPDATE listing_sources
         SET crawl_rate_limited_until = NOW() - INTERVAL '1 minute'
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let candidate = due_spider_candidate(&pool, &service, listing_source_id)
        .await
        .expect("a rate-limited ListingSource is due again after its cooldown");
    assert_eq!(
        candidate.crawl_rate_limit_streak, 1,
        "a crawl cut short without a rate limit steps the pace up by one level"
    );

    service
        .mark_crawl_failure(
            &root,
            "ConnectError",
            2,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let candidate = due_spider_candidate(&pool, &service, listing_source_id)
        .await
        .unwrap();
    assert_eq!(
        candidate.crawl_rate_limit_streak, 1,
        "a failure keeps the pace"
    );

    service
        .mark_crawl_completed(&root, CrawlCompletion::Completed)
        .await
        .unwrap();
    let candidate = due_spider_candidate(&pool, &service, listing_source_id)
        .await
        .expect("the ListingSource stays selectable in this test, which never marks it crawled");
    assert_eq!(
        candidate.crawl_rate_limit_streak, 0,
        "a full crawl restores the configured pace"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_rate_limit_state_should_survive_a_review_releasing_the_source() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "spider-rate-limited-review.example.com",
    )
    .await;
    let root = crawled_root(listing_source_id, "spider-rate-limited-review.example.com");
    service
        .mark_crawl_rate_limited(
            &root,
            CrawlRateLimitWrite {
                streak: 1,
                not_before: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
            },
        )
        .await
        .unwrap();

    // What a review decision releasing the ListingSource writes.
    sqlx::query("UPDATE listing_sources SET next_crawl_at = NULL WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        !service
            .get_candidates(10, &[])
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.listing_source_id == listing_source_id),
        "the shop's limit still holds the ListingSource back"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_rate_limit_streak_should_reject_a_negative_value() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "spider-rate-limit-check.example.com",
    )
    .await;
    let root = crawled_root(listing_source_id, "spider-rate-limit-check.example.com");

    let written = service
        .mark_crawl_rate_limited(
            &root,
            CrawlRateLimitWrite {
                streak: -1,
                not_before: time::OffsetDateTime::now_utc(),
            },
        )
        .await;

    assert!(written.is_err(), "{written:?}");
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_truncated_should_keep_failure_metadata_and_defer_retry() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-truncated.example.com").await;
    let root = crawled_root(listing_source_id, "spider-truncated.example.com");
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    service
        .mark_crawl_failure(&root, "RateLimited", 2, None, past)
        .await
        .unwrap();
    assert_eq!(
        crawl_state(&pool, listing_source_id).await.3.as_deref(),
        Some("FAILED")
    );

    let next_crawl_at = time::OffsetDateTime::now_utc() + time::Duration::hours(24);
    service
        .mark_crawl_truncated(&root, 1, next_crawl_at)
        .await
        .unwrap();

    let (failure_count, error_kind, stored_next, outcome, truncations, last_crawled) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!(failure_count, 2);
    assert_eq!(error_kind.as_deref(), Some("RateLimited"));
    assert_eq!(outcome.as_deref(), Some("HARD_TIMEOUT_TRUNCATED"));
    assert_eq!(truncations, 1);
    assert!(
        last_crawled.is_none(),
        "a truncated crawl is not a full crawl"
    );
    assert!((stored_next.unwrap() - next_crawl_at).whole_seconds().abs() <= 1);
    assert!(
        !service
            .get_candidates(10, &[])
            .await
            .unwrap()
            .iter()
            .any(|c| c.listing_source_id == listing_source_id),
        "a truncated ListingSource waits for its follow-up cooldown"
    );

    sqlx::query(
        "UPDATE listing_sources SET next_crawl_at = NOW() - INTERVAL '1 minute' WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let candidate = service
        .get_candidates(10, &[])
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.listing_source_id == listing_source_id)
        .expect("a truncated ListingSource is due again after its cooldown");
    assert_eq!(candidate.crawl_truncation_count, 1);
    assert_eq!(candidate.crawl_failure_count, 2);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_completed_should_clear_failure_and_truncation_state() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-at-budget.example.com").await;
    let root = crawled_root(listing_source_id, "spider-at-budget.example.com");
    let future = time::OffsetDateTime::now_utc() + time::Duration::hours(24);
    service
        .mark_crawl_failure(&root, "ConnectError", 1, None, future)
        .await
        .unwrap();
    service
        .mark_crawl_truncated(&root, 3, future)
        .await
        .unwrap();

    service
        .mark_crawl_completed(&root, CrawlCompletion::PageBudgetReached)
        .await
        .unwrap();

    let (failure_count, error_kind, next_crawl_at, outcome, truncations, _) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!(failure_count, 0);
    assert_eq!(error_kind, None);
    assert_eq!(next_crawl_at, None);
    assert_eq!(outcome.as_deref(), Some("PAGE_BUDGET_REACHED"));
    assert_eq!(truncations, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_completed_should_reject_truncated_outcome() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "spider-misused-completed.example.com",
    )
    .await;
    let root = crawled_root(listing_source_id, "spider-misused-completed.example.com");
    let future = time::OffsetDateTime::now_utc() + time::Duration::hours(6);
    service
        .mark_crawl_failure(&root, "RateLimited", 2, None, future)
        .await
        .unwrap();

    let result = service
        .mark_crawl_completed(&root, CrawlCompletion::HardTimeoutTruncated)
        .await;

    assert!(result.is_err());
    let (failure_count, error_kind, ..) = crawl_state(&pool, listing_source_id).await;
    assert_eq!(failure_count, 2);
    assert_eq!(error_kind.as_deref(), Some("RateLimited"));
}

async fn insert_pending_url_pattern_review(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> CrawlerReviewId {
    let review_id = CrawlerReviewId::new();
    sqlx::query(
        "INSERT INTO crawler_reviews (review_id, listing_source_id, artifact_type, status, reason, candidate_payload) \
         VALUES ($1, $2, 'URL_PATTERN', 'PENDING_REVIEW', 'manual_url_pattern_required', '{}'::jsonb)",
    )
    .bind(review_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    review_id
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_awaiting_url_pattern_review_should_keep_failure_state_and_defer() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-review-wait.example.com").await;
    let root = crawled_root(listing_source_id, "spider-review-wait.example.com");
    let review_id = insert_pending_url_pattern_review(&pool, listing_source_id).await;
    let past = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
    service
        .mark_crawl_failure(&root, "RateLimited", 2, None, past)
        .await
        .unwrap();

    let next_crawl_at = time::OffsetDateTime::now_utc() + time::Duration::days(7);
    let wait_write = service
        .mark_crawl_awaiting_url_pattern_review(&root, &review_id, None, next_crawl_at)
        .await
        .unwrap();

    assert_eq!(wait_write, UrlPatternReviewWaitWrite::Recorded);
    let (failure_count, error_kind, stored_next, outcome, truncations, last_crawled) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!(failure_count, 2, "a review wait is not a failure");
    assert_eq!(error_kind.as_deref(), Some("RateLimited"));
    assert_eq!(
        outcome.as_deref(),
        Some(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
    );
    assert_eq!(truncations, 0);
    assert!(last_crawled.is_none());
    assert!((stored_next.unwrap() - next_crawl_at).whole_seconds().abs() <= 1);
    assert!(
        !service
            .get_candidates(10, &[])
            .await
            .unwrap()
            .iter()
            .any(|c| c.listing_source_id == listing_source_id),
        "a ListingSource waiting for a review is not crawled until the review is decided"
    );
}

/// A ListingSource waits only on a pending URL-pattern review of its own; a wait on any other
/// review leaves its crawl state as it was.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_record_a_url_pattern_review_wait_only_on_a_pending_review_of_the_source() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-wait-guard.example.com").await;
    let root = crawled_root(listing_source_id, "spider-wait-guard.example.com");
    let other_listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        other_listing_source_id,
        "spider-other-guard.example.com",
    )
    .await;
    let other_source_review_id =
        insert_pending_url_pattern_review(&pool, other_listing_source_id).await;
    let crawl_state_before = crawl_state(&pool, listing_source_id).await;
    let next_crawl_at = time::OffsetDateTime::now_utc() + time::Duration::days(7);

    for (review_id, case) in [
        (CrawlerReviewId::new(), "an unknown review"),
        (other_source_review_id, "another ListingSource's review"),
    ] {
        assert_eq!(
            service
                .mark_crawl_awaiting_url_pattern_review(&root, &review_id, None, next_crawl_at)
                .await
                .unwrap(),
            UrlPatternReviewWaitWrite::ReviewNotPending,
            "a ListingSource must not wait on {case}"
        );
    }

    assert_eq!(
        crawl_state(&pool, listing_source_id).await,
        crawl_state_before
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_mark_crawl_needs_operator_should_take_source_out_of_the_schedule() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "spider-needs-operator.example.com",
    )
    .await;
    let root = crawled_root(listing_source_id, "spider-needs-operator.example.com");

    service
        .mark_crawl_needs_operator(&root, "NoUrlPattern", 5)
        .await
        .unwrap();

    let (failure_count, error_kind, next_crawl_at, outcome, ..) =
        crawl_state(&pool, listing_source_id).await;
    assert_eq!(failure_count, 5);
    assert_eq!(error_kind.as_deref(), Some("NoUrlPattern"));
    assert_eq!(
        next_crawl_at, None,
        "no cooldown is pending; the state is terminal"
    );
    assert_eq!(outcome.as_deref(), Some(NEEDS_OPERATOR_CRAWL_OUTCOME));
    assert!(
        !service
            .get_candidates(10, &[])
            .await
            .unwrap()
            .iter()
            .any(|c| c.listing_source_id == listing_source_id),
        "a ListingSource waiting for an operator is never selected"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_crawl_outcome_should_reject_unknown_values() {
    let pool = get_postgres_client().await;

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "spider-bad-outcome.example.com").await;

    let result = sqlx::query(
        "UPDATE listing_sources SET last_crawl_outcome = 'Truncated' WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "outcome values must stay in the persisted contract"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — ordering: least recently run first, never fully crawled first among equals
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_order_never_crawled_before_stale_crawled() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());

    let listing_source_id_stale = ListingSourceId::new();
    let listing_source_id_never = ListingSourceId::new();

    insert_listing_source(
        &pool,
        listing_source_id_stale,
        "spider-order-stale.example.com",
    )
    .await;
    insert_listing_source(
        &pool,
        listing_source_id_never,
        "spider-order-never.example.com",
    )
    .await;

    sqlx::query(
        "UPDATE listing_sources SET last_crawled = NOW() - INTERVAL '10 days' WHERE listing_source_id = $1",
    )
    .bind(listing_source_id_stale.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(100, &[]).await.unwrap();

    let pos_never = candidates
        .iter()
        .position(|c| c.listing_source_id == listing_source_id_never)
        .expect("never-crawled ListingSource should be in candidates");
    let pos_stale = candidates
        .iter()
        .position(|c| c.listing_source_id == listing_source_id_stale)
        .expect("stale ListingSource should be in candidates");

    assert!(
        pos_never < pos_stale,
        "never-crawled (pos {pos_never}) should come before stale (pos {pos_stale})"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn spider_should_order_a_due_incremental_crawl_before_a_recent_truncated_continuation() {
    let pool = get_postgres_client().await;
    let service = SpiderCandidateServiceImpl::new(pool.clone());
    let continuation = ListingSourceId::new();
    insert_listing_source(&pool, continuation, "spider-order-continuation.example.com").await;
    let incremental = ListingSourceId::new();
    insert_listing_source(&pool, incremental, "spider-order-incremental.example.com").await;
    // Never fully crawled, truncated six hours ago and due again.
    sqlx::query(
        "UPDATE listing_sources \
         SET last_crawl_run_at = NOW() - INTERVAL '6 hours', crawl_truncation_count = 3, \
             last_crawl_outcome = 'HARD_TIMEOUT_TRUNCATED', \
             next_crawl_at = NOW() - INTERVAL '1 minute' \
         WHERE listing_source_id = $1",
    )
    .bind(continuation.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE listing_sources \
         SET last_crawled = NOW() - INTERVAL '3 days', \
             last_crawl_run_at = NOW() - INTERVAL '25 hours' \
         WHERE listing_source_id = $1",
    )
    .bind(incremental.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let first: Vec<(ListingSourceId, CrawlKind)> = service
        .get_candidates(1, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|candidate| (candidate.listing_source_id, candidate.kind))
        .collect();

    assert_eq!(first, vec![(incremental, CrawlKind::Incremental)]);
    let next: Vec<ListingSourceId> = service
        .get_candidates(1, &[incremental])
        .await
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.listing_source_id)
        .collect();
    assert_eq!(next, vec![continuation]);
}

// ============================================================================
// SCRAPER CANDIDATE SERVICE
// ============================================================================

/// Helper: inserts a product URL into listing_source_urls for the given ListingSource.
async fn insert_product_url(pool: &sqlx::PgPool, listing_source_id: ListingSourceId, url: &str) {
    let parsed = url::Url::parse(url).unwrap();
    let repo = UrlMetadataRepositoryImpl::new(pool.clone());
    repo.upsert_link(&listing_source_id, &parsed, &UrlClass::ProductListing)
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// get_candidates — a pending product-schema review parks only a source without schemas
// ---------------------------------------------------------------------------

async fn insert_pending_product_schema_review(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> CrawlerReviewId {
    let review_id = CrawlerReviewId::new();
    sqlx::query(
        "INSERT INTO crawler_reviews (review_id, listing_source_id, artifact_type, status, reason, candidate_payload) \
         VALUES ($1, $2, 'PRODUCT_SCHEMA', 'PENDING_REVIEW', 'manual_schema_required', '{\"schemas\": []}'::jsonb)",
    )
    .bind(review_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    review_id
}

/// Stores `product_schema` as the source's cached schema row; the candidate query reads only
/// whether the row holds a schema.
async fn insert_cached_product_schemas(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    product_schema: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(product_schema)
    .execute(pool)
    .await
    .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_return_candidate_when_schema_review_is_pending_and_schemas_are_cached() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-review.example.com").await;
    insert_product_url(
        &pool,
        listing_source_id,
        "https://scraper-review.example.com/p/1",
    )
    .await;
    insert_cached_product_schemas(
        &pool,
        listing_source_id,
        serde_json::json!([{ "title": { "type": "text", "selector": "h1", "cardinality": "first" } }]),
    )
    .await;
    insert_pending_product_schema_review(&pool, listing_source_id).await;

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        candidates
            .iter()
            .any(|c| c.url.as_str() == "https://scraper-review.example.com/p/1"),
        "a pending review must not park a source whose cached schemas can still extract pages"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_return_candidate_when_schema_review_is_pending_without_cached_schemas()
{
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-initial.example.com").await;
    insert_product_url(
        &pool,
        listing_source_id,
        "https://scraper-initial.example.com/p/1",
    )
    .await;
    insert_pending_product_schema_review(&pool, listing_source_id).await;

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        candidates
            .iter()
            .all(|c| c.listing_source_id != listing_source_id),
        "a source without cached schemas waits on its initial schema review"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_return_candidate_when_schema_review_is_pending_and_schema_row_is_empty()
{
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-empty.example.com").await;
    insert_product_url(
        &pool,
        listing_source_id,
        "https://scraper-empty.example.com/p/1",
    )
    .await;
    insert_cached_product_schemas(&pool, listing_source_id, serde_json::json!([])).await;
    insert_pending_product_schema_review(&pool, listing_source_id).await;

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        candidates
            .iter()
            .all(|c| c.listing_source_id != listing_source_id),
        "an empty schema row extracts nothing, so the source waits on its review"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — URL scraped > 1 day ago IS returned
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_return_candidate_when_scraped_more_than_1_day_ago() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-stale.example.com").await;
    let url = "https://scraper-stale.example.com/p/1";
    insert_product_url(&pool, listing_source_id, url).await;

    // Mark as scraped 2 days ago to isolate the time-based filter.
    let old_hash = "old_hash_differs";
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days', last_scraped_hash = $1 WHERE url = $2",
    )
    .bind(old_hash)
    .bind(url)
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        candidates.iter().any(|c| c.url.as_str() == url),
        "URL scraped 2 days ago should be returned"
    );
}

/// The guard reads only a confirmation that is still in the future, so an expired one, a revoked
/// one and an unknown source all read as none.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mass_removal_confirmed_until_should_read_only_a_live_confirmation() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "confirmed.example.com").await;
    let set_confirmation = |seconds_from_now: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "UPDATE listing_sources \
                 SET mass_removal_confirmed_until = NOW() + make_interval(secs => $2), \
                     mass_removal_confirmed_by = 'HUMAN' \
                 WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
            .bind(seconds_from_now)
            .execute(&pool)
            .await
            .unwrap();
        }
    };

    assert_eq!(
        service
            .mass_removal_confirmed_until(&listing_source_id)
            .await
            .unwrap(),
        None
    );
    set_confirmation(3600).await;
    assert!(
        service
            .mass_removal_confirmed_until(&listing_source_id)
            .await
            .unwrap()
            .is_some()
    );
    set_confirmation(-1).await;
    assert_eq!(
        service
            .mass_removal_confirmed_until(&listing_source_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        service
            .mass_removal_confirmed_until(&ListingSourceId::new())
            .await
            .unwrap(),
        None
    );
}

// ---------------------------------------------------------------------------
// count_due_urls — counts exactly the URLs candidate selection could return
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_count_due_urls_by_the_candidate_eligibility_rule() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-due.example.com").await;
    for index in 1..=3 {
        insert_product_url(
            &pool,
            listing_source_id,
            &format!("https://scraper-due.example.com/p/{index}"),
        )
        .await;
    }
    sqlx::query("UPDATE listing_source_urls SET last_scraped = NOW() WHERE url = $1")
        .bind("https://scraper-due.example.com/p/3")
        .execute(&pool)
        .await
        .unwrap();

    let due = service.count_due_urls().await.unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert_eq!(due, 2);
    assert_eq!(due, candidates.len() as u64);
}

// ---------------------------------------------------------------------------
// sources_awaiting_review — the ListingSources only a pending review keeps from scraping
// ---------------------------------------------------------------------------

async fn match_url_pattern(pool: &sqlx::PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "UPDATE listing_sources SET url_pattern = '/p/', url_pattern_state = 'MATCHED', url_pattern_matching = 'FULL_URL_SEARCH' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_list_only_sources_whose_scraping_a_pending_review_holds_back() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    // The ListingSource waits on its first product schema.
    let first_schema = ListingSourceId::new();
    insert_listing_source(&pool, first_schema, "first-schema.example.com").await;
    match_url_pattern(&pool, first_schema).await;
    insert_pending_product_schema_review(&pool, first_schema).await;
    // The cached schema still extracts pages, so the scraper scrapes them.
    let cached_schema = ListingSourceId::new();
    insert_listing_source(&pool, cached_schema, "cached-schema.example.com").await;
    insert_cached_product_schemas(
        &pool,
        cached_schema,
        serde_json::json!([{ "title": { "type": "text", "selector": "h1", "cardinality": "first" } }]),
    )
    .await;
    insert_pending_product_schema_review(&pool, cached_schema).await;
    // Without a pattern the ListingSource has no product URL until its review is decided.
    let pattern_review = ListingSourceId::new();
    insert_listing_source(&pool, pattern_review, "pattern-review.example.com").await;
    insert_pending_url_pattern_review(&pool, pattern_review).await;
    // A restored pattern is live while a newer URL-pattern review is pending.
    let matched = ListingSourceId::new();
    insert_listing_source(&pool, matched, "matched-pattern.example.com").await;
    match_url_pattern(&pool, matched).await;
    insert_pending_url_pattern_review(&pool, matched).await;
    // A disabled ListingSource is not scraped whatever its reviews.
    let disabled = ListingSourceId::new();
    insert_listing_source(&pool, disabled, "disabled.example.com").await;
    insert_pending_url_pattern_review(&pool, disabled).await;
    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(disabled.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    // An open scrape circuit keeps the ListingSource from scraping as well.
    let cooling = ListingSourceId::new();
    insert_listing_source(&pool, cooling, "cooling-circuit.example.com").await;
    insert_pending_url_pattern_review(&pool, cooling).await;
    sqlx::query(
        "UPDATE listing_sources \
         SET scrape_failure_streak = 3, last_scrape_error_kind = 'HTTP_503', \
             next_scrape_at = NOW() + INTERVAL '1 hour' \
         WHERE listing_source_id = $1",
    )
    .bind(cooling.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    // No review is pending.
    insert_listing_source(&pool, ListingSourceId::new(), "no-review.example.com").await;

    let sources = service.sources_awaiting_review().await.unwrap();

    assert_eq!(
        sources,
        vec![
            SourceAwaitingReview {
                listing_source_id: first_schema,
                crawl_host: "first-schema.example.com".to_owned(),
            },
            SourceAwaitingReview {
                listing_source_id: pattern_review,
                crawl_host: "pattern-review.example.com".to_owned(),
            },
        ]
    );
}

// ---------------------------------------------------------------------------
// get_candidates — non-product URL class is NOT returned
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_return_candidate_when_url_class_is_not_product() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-class.example.com").await;

    // Insert as a category URL (not product)
    let url = url::Url::parse("https://scraper-class.example.com/category/1").unwrap();
    let repo = UrlMetadataRepositoryImpl::new(pool.clone());
    repo.upsert_link(&listing_source_id, &url, &UrlClass::Category)
        .await
        .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        !candidates.iter().any(|c| c.url == url),
        "category URL should not be returned as a scraper candidate"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — inactive ListingSource is NOT returned
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_persist_url_class_other_and_exclude_candidate_when_set_class_is_called() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-set-class.example.com").await;
    let target_url = url::Url::parse("https://scraper-set-class.example.com/p/target").unwrap();
    let other_url = url::Url::parse("https://scraper-set-class.example.com/p/other").unwrap();
    let repo = UrlMetadataRepositoryImpl::new(pool.clone());
    repo.upsert_link(&listing_source_id, &target_url, &UrlClass::ProductListing)
        .await
        .unwrap();
    repo.upsert_link(&listing_source_id, &other_url, &UrlClass::ProductListing)
        .await
        .unwrap();

    assert_eq!(
        service
            .set_class(&listing_source_id, &target_url, UrlClass::Other, None)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let target_class: String = sqlx::query_scalar(
        "SELECT url_class FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(target_url.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_class: String = sqlx::query_scalar(
        "SELECT url_class FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(other_url.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert_eq!(target_class, UrlClass::Other.to_string());
    assert_eq!(other_class, UrlClass::ProductListing.to_string());
    assert!(!candidates.iter().any(|c| c.url == target_url));
    assert!(candidates.iter().any(|c| c.url == other_url));
}

#[derive(sqlx::FromRow)]
struct ScrapeOutcomeRow {
    url_class: String,
    last_error_kind: Option<String>,
    last_error_message: Option<String>,
    failure_count: i32,
    last_status_code: Option<i32>,
    retry_scheduled: bool,
    scraped_now: bool,
    last_scraped_hash: Option<String>,
}

/// A page evaluated without a capture keeps its product class and its outcome, waits for
/// the normal scrape interval instead of a retry, and keeps the page hash of its last
/// capture so the next scrape evaluates it again.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_scraped_without_capture_should_record_outcome_until_the_scrape_interval() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-untitled.example.com").await;
    let url =
        url::Url::parse("https://scraper-untitled.example.com/furniture/item-1208922").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    sqlx::query(
        "UPDATE listing_source_urls \
         SET failure_count = 2, last_status_code = 503, last_error_kind = 'HTTP_503', \
             last_scraped_hash = 'hash-of-last-capture' \
         WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let message = "Required value missing from 'https://scraper-untitled.example.com/furniture/item-1208922': `title` is blank";

    let stale = service
        .mark_scraped_without_capture(
            &listing_source_id,
            &url,
            "MissingRequiredValue",
            message,
            Some(&raw_input_hash()),
        )
        .await
        .unwrap();
    let applied = service
        .mark_scraped_without_capture(
            &listing_source_id,
            &url,
            "MissingRequiredValue",
            message,
            None,
        )
        .await
        .unwrap();

    assert_eq!(stale, CrawlerUrlWriteOutcome::NoopStale);
    assert_eq!(applied, CrawlerUrlWriteOutcome::Applied);
    let row = sqlx::query_as::<_, ScrapeOutcomeRow>(
        "SELECT url_class, last_error_kind, last_error_message, failure_count, last_status_code, \
                next_retry_at IS NOT NULL AS retry_scheduled, \
                last_scraped > NOW() - INTERVAL '1 minute' AS scraped_now, last_scraped_hash \
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.url_class, UrlClass::ProductListing.to_string());
    assert_eq!(row.last_error_kind.as_deref(), Some("MissingRequiredValue"));
    assert_eq!(row.last_error_message.as_deref(), Some(message));
    assert_eq!(row.failure_count, 0);
    assert_eq!(row.last_status_code, None);
    assert!(!row.retry_scheduled);
    assert!(row.scraped_now);
    assert_eq!(
        row.last_scraped_hash.as_deref(),
        Some("hash-of-last-capture")
    );
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    assert!(!candidates.iter().any(|c| c.url == url));
}

/// An unsettled redirect holds the URL back until its recheck time and comes back with the
/// candidate, so the next scrape continues its streak; another outcome ends the streak.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_unsettled_redirect_should_hold_the_url_and_carry_its_streak() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-redirect.example.com").await;
    let url = url::Url::parse("https://scraper-redirect.example.com/producto.aspx?id=589").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    let error_page = url::Url::parse(
        "https://scraper-redirect.example.com/error.aspx?aspxerrorpath=/producto.aspx",
    )
    .unwrap();
    let redirect = UnsettledRedirect {
        destination: error_page.clone(),
        streak: 3,
    };
    sqlx::query("UPDATE listing_source_urls SET page_error_streak = 4 WHERE url = $1")
        .bind(url.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let applied = service
        .mark_unsettled_redirect(UnsettledRedirectRequest {
            listing_source_id,
            url: url.clone(),
            outcome_kind: "RedirectTargetUnverified".to_owned(),
            outcome_message: "redirects outside its product URL pattern".to_owned(),
            redirect: redirect.clone(),
            next_retry_at: time::OffsetDateTime::now_utc() + time::Duration::days(4),
            expected_last_captured_raw_input_sha256: None,
        })
        .await
        .unwrap();

    assert_eq!(applied, CrawlerUrlWriteOutcome::Applied);
    let page_error_streak: i32 =
        sqlx::query_scalar("SELECT page_error_streak FROM listing_source_urls WHERE url = $1")
            .bind(url.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        page_error_streak, 0,
        "a fetch that redirected ends the page-error run"
    );
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    assert!(!candidates.iter().any(|c| c.url == url), "held back");

    sqlx::query(
        "UPDATE listing_source_urls \
         SET last_scraped = NOW() - INTERVAL '5 days', next_retry_at = NOW() - INTERVAL '1 minute' \
         WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    let candidate = candidates.iter().find(|c| c.url == url).expect("due again");
    assert_eq!(candidate.unsettled_redirect.as_ref(), Some(&redirect));

    service
        .mark_scraped_without_capture(
            &listing_source_id,
            &url,
            "MissingRequiredValue",
            "title is blank",
            None,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    let candidate = candidates.iter().find(|c| c.url == url).expect("due again");
    assert_eq!(candidate.unsettled_redirect, None);
}

/// A URL whose redirect target no cached schema could judge is due again at its scrape interval
/// once its ListingSource stores a product schema, whatever stored it, instead of waiting out its
/// recheck delay.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_release_an_unverified_redirect_once_a_product_schema_is_stored() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-unverified.example.com").await;
    let url = url::Url::parse("https://scraper-unverified.example.com/producto.aspx?id=7").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    service
        .mark_unsettled_redirect(UnsettledRedirectRequest {
            listing_source_id,
            url: url.clone(),
            outcome_kind: "RedirectTargetUnverified".to_owned(),
            outcome_message: "no cached schema judges that page".to_owned(),
            redirect: UnsettledRedirect {
                destination: url::Url::parse("https://scraper-unverified.example.com/error.aspx")
                    .unwrap(),
                streak: 4,
            },
            next_retry_at: time::OffsetDateTime::now_utc() + time::Duration::days(8),
            expected_last_captured_raw_input_sha256: None,
        })
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    let held = service.get_candidates(10, 100, &[]).await.unwrap();

    insert_cached_product_schemas(
        &pool,
        listing_source_id,
        serde_json::json!([{ "title": { "type": "text", "selector": "h1", "cardinality": "first" } }]),
    )
    .await;
    let released = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(!held.iter().any(|c| c.url == url), "held without a schema");
    assert!(
        released.iter().any(|c| c.url == url),
        "due once a schema is stored"
    );
}

/// The product URLs that stand at a redirect target are those whose last scrape ended there
/// unverified and those whose last removal was captured on it; the removal keeps its target.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_count_the_urls_at_a_redirect_target_through_their_removal() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-shared.example.com").await;
    let error_page = url::Url::parse("https://scraper-shared.example.com/error.aspx").unwrap();
    let urls: Vec<url::Url> = (1..=4)
        .map(|id| {
            url::Url::parse(&format!(
                "https://scraper-shared.example.com/producto.aspx?id={id}"
            ))
            .unwrap()
        })
        .collect();
    for url in &urls {
        insert_product_url(&pool, listing_source_id, url.as_str()).await;
    }
    for (url, kind) in [
        (&urls[0], "RedirectTargetUnverified"),
        (&urls[1], "RedirectTargetUnverified"),
        (&urls[2], "RedirectDestinationUnsettled"),
    ] {
        service
            .mark_unsettled_redirect(UnsettledRedirectRequest {
                listing_source_id,
                url: url.clone(),
                outcome_kind: kind.to_owned(),
                outcome_message: "unsettled".to_owned(),
                redirect: UnsettledRedirect {
                    destination: error_page.clone(),
                    streak: 1,
                },
                next_retry_at: time::OffsetDateTime::now_utc() + time::Duration::days(1),
                expected_last_captured_raw_input_sha256: None,
            })
            .await
            .unwrap();
    }
    let removed = service
        .mark_removed(
            &listing_source_id,
            &urls[3],
            &[7; 32],
            None,
            &[],
            Some(&error_page),
        )
        .await
        .unwrap();

    let at_target = service
        .count_urls_at_redirect_target(&listing_source_id, &error_page, &urls[0], 100)
        .await
        .unwrap();
    let capped = service
        .count_urls_at_redirect_target(&listing_source_id, &error_page, &urls[0], 1)
        .await
        .unwrap();

    assert_eq!(removed, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(at_target, 2, "the other unverified URL and the removed one");
    assert_eq!(capped, 1);
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1",
    )
    .bind(urls[3].as_str())
    .execute(&pool)
    .await
    .unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    let removed_candidate = candidates
        .iter()
        .find(|c| c.url == urls[3])
        .expect("a removal stays a candidate");
    assert_eq!(
        removed_candidate.removed_by_redirect_target.as_ref(),
        Some(&error_page)
    );
}

/// A shared-redirect removal the mass-removal guard held keeps its redirect: the URL comes back
/// with its streak and counts at the target. A held removal of another kind, written without a
/// redirect, ends the streak instead of reviving it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_keep_the_redirect_of_a_held_shared_redirect_removal() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-held.example.com").await;
    let error_page = url::Url::parse("https://scraper-held.example.com/error.aspx").unwrap();
    let held = url::Url::parse("https://scraper-held.example.com/producto.aspx?id=1").unwrap();
    let other = url::Url::parse("https://scraper-held.example.com/producto.aspx?id=2").unwrap();
    for url in [&held, &other] {
        insert_product_url(&pool, listing_source_id, url.as_str()).await;
    }
    let redirect = UnsettledRedirect {
        destination: error_page.clone(),
        streak: 2,
    };
    service
        .mark_unsettled_redirect(UnsettledRedirectRequest {
            listing_source_id,
            url: held.clone(),
            outcome_kind: "MassRemovalHeld".to_owned(),
            outcome_message: "removal held by the mass-removal guard".to_owned(),
            redirect: redirect.clone(),
            next_retry_at: time::OffsetDateTime::now_utc(),
            expected_last_captured_raw_input_sha256: None,
        })
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NULL WHERE url = $1 AND last_error_kind = 'MassRemovalHeld'",
    )
    .bind(held.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    let candidate = candidates
        .iter()
        .find(|c| c.url == held)
        .expect("held URL is due");
    assert_eq!(candidate.unsettled_redirect.as_ref(), Some(&redirect));
    assert_eq!(
        service
            .count_urls_at_redirect_target(&listing_source_id, &error_page, &other, 100)
            .await
            .unwrap(),
        1
    );

    service
        .mark_scraped_without_capture(
            &listing_source_id,
            &held,
            "MassRemovalHeld",
            "removal held by the mass-removal guard",
            None,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE listing_source_urls SET last_scraped = NULL WHERE url = $1")
        .bind(held.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    let candidate = candidates
        .iter()
        .find(|c| c.url == held)
        .expect("held URL is due");
    assert_eq!(candidate.unsettled_redirect, None);
    assert_eq!(
        service
            .count_urls_at_redirect_target(&listing_source_id, &error_page, &other, 100)
            .await
            .unwrap(),
        0
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_return_candidate_when_listing_source_crawl_is_disabled() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-inactive.example.com").await;
    let url = "https://scraper-inactive.example.com/p/1";
    insert_product_url(&pool, listing_source_id, url).await;

    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        !candidates.iter().any(|c| c.url.as_str() == url),
        "candidate for crawl-disabled listing source should be excluded"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — sold URLs remain eligible for rechecks
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_return_candidate_when_disposition_is_dormant_sold() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-disposition.example.com").await;
    let sold_url = url::Url::parse("https://scraper-disposition.example.com/p/sold").unwrap();
    let present_url =
        url::Url::parse("https://scraper-disposition.example.com/p/out-of-stock").unwrap();
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    for url in [&sold_url, &present_url] {
        repository
            .upsert_link(&listing_source_id, url, &UrlClass::ProductListing)
            .await
            .unwrap();
    }
    repository
        .set_disposition(
            &listing_source_id,
            &sold_url,
            CrawlerDisposition::DormantSold,
        )
        .await
        .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();

    assert!(
        candidates.iter().any(|candidate| candidate.url == sold_url),
        "sold URLs must remain candidates for removal/restock rechecks"
    );
    assert!(
        candidates
            .iter()
            .any(|candidate| candidate.url == present_url)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_removed_should_reactivate_sold_url_after_durable_capture() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-removed-reactivation.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-removed-reactivation.example.com/p/1").unwrap();
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());
    repository
        .upsert_link(&listing_source_id, &url, &UrlClass::ProductListing)
        .await
        .unwrap();
    repository
        .set_disposition(&listing_source_id, &url, CrawlerDisposition::DormantSold)
        .await
        .unwrap();

    let removal_hash = vec![7; 32];
    assert_eq!(
        service
            .mark_removed(&listing_source_id, &url, &removal_hash, None, &[], None)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let row: (String, Option<Vec<u8>>, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT crawler_disposition, last_captured_raw_input_sha256, last_scraped \
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, CrawlerDisposition::Active.as_str());
    assert_eq!(row.1, Some(removal_hash));
    assert!(row.2.is_some());
    assert!(
        !service
            .get_candidates(10, 100, &[])
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.url == url),
        "the removal scrape must obey the usual daily revisit cadence"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_removed_should_forget_the_page_hash_of_the_last_product_scrape() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-removed-page-hash.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-removed-page-hash.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    let product_capture = raw_input_hash();
    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            url.clone(),
            (
                "p".repeat(64),
                "schema-fingerprint",
                product_capture.clone(),
                "SKU-123",
                CrawlerDisposition::Active,
                None,
            ),
        ))
        .await
        .unwrap();

    assert_eq!(
        service
            .mark_removed(
                &listing_source_id,
                &url,
                &[9; 32],
                Some(&product_capture),
                &[],
                None
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    // A page that comes back unchanged after its removal must not match the page hash of the
    // product scrape before it, or the scraper would skip it and leave the listing removed.
    // Nor may it carry the removed listing's title forward if it comes back without one.
    let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT last_scraped_hash, last_scraped_schema_fingerprint, last_captured_title \
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, None, None));
}

/// A template removal records the templates that captured it, and the next product capture of
/// the page forgets them, so only a page a template still holds removed is re-armed when it is
/// retired.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_removed_should_record_the_templates_until_a_product_capture() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-removed-by-templates.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-removed-by-templates.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    let template = crawler::scraper::css_selector::removed_page_schema::RemovedPageSchema {
        selector: "p.notice".into(),
        text: Some("no longer available".to_owned()),
        regex: None,
    };
    let removal = [7; 32];
    service
        .mark_removed(
            &listing_source_id,
            &url,
            &removal,
            None,
            std::slice::from_ref(&template),
            None,
        )
        .await
        .unwrap();
    let recorded = || async {
        sqlx::query_scalar::<_, Option<serde_json::Value>>(
            "SELECT removed_by_templates FROM listing_source_urls WHERE url = $1",
        )
        .bind(url.as_str())
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(recorded().await, Some(serde_json::json!([template])));

    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            url.clone(),
            (
                "p".repeat(64),
                "schema-fingerprint",
                raw_input_hash(),
                "SKU-123",
                CrawlerDisposition::Active,
                Some(removal.to_vec()),
            ),
        ))
        .await
        .unwrap();

    assert_eq!(recorded().await, None);
}

/// The title a product capture recorded reaches the next scrape of the URL, which carries it
/// forward if the shop blanks the name (docs/crawler/extraction-and-review.md).
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_candidate_should_carry_the_title_of_the_last_product_capture() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-captured-title.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-captured-title.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    assert_eq!(candidate_title(&service, &url).await, Some(None));

    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            url.clone(),
            (
                "p".repeat(64),
                "schema-fingerprint",
                raw_input_hash(),
                "SKU-123",
                CrawlerDisposition::Active,
                None,
            ),
        ))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1",
    )
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        candidate_title(&service, &url).await,
        Some(Some("Captured title".to_owned()))
    );
}

/// The `last_captured_title` of `url`'s scraper candidate, `None` when it is not due.
async fn candidate_title(
    service: &ScraperCandidateServiceImpl,
    url: &url::Url,
) -> Option<Option<String>> {
    service
        .get_candidates(10, 10, &[])
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.url == *url)
        .map(|candidate| candidate.last_captured_title)
}

/// A removal whose capture commits but whose `mark_removed` write fails leaves the URL as
/// `forget_capture_before_removal` left it, so the product page that returns unchanged is
/// extracted and its capture completes against that state.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_forget_capture_before_removal_should_not_leave_the_url_on_the_product_capture() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-removal-forget.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-removal-forget.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    let product_capture = raw_input_hash();
    let product_completion = || {
        scrape_completion(
            listing_source_id,
            url.clone(),
            (
                "p".repeat(64),
                "schema-fingerprint",
                product_capture.clone(),
                "SKU-123",
                CrawlerDisposition::Active,
                None,
            ),
        )
    };
    service.mark_as_scraped(product_completion()).await.unwrap();

    assert_eq!(
        service
            .forget_capture_before_removal(&listing_source_id, &url, Some(&[1; 32]))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale,
        "the write is fenced on the capture the removal observed"
    );
    assert_eq!(
        service
            .forget_capture_before_removal(&listing_source_id, &url, Some(&product_capture))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let row: (
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT last_scraped_hash, last_scraped_schema_fingerprint, \
         last_captured_raw_input_sha256, last_captured_title \
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, None, None, None));
    assert_eq!(
        service.mark_as_scraped(product_completion()).await.unwrap(),
        CrawlerUrlWriteOutcome::Applied,
        "the returning page's capture completes against the forgotten state"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — limit is respected
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_respect_limit_when_multiple_candidates_exist() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-limit.example.com").await;

    for i in 0..5u32 {
        insert_product_url(
            &pool,
            listing_source_id,
            &format!("https://scraper-limit.example.com/p/{}", i),
        )
        .await;
    }

    let candidates = service.get_candidates(10, 3, &[]).await.unwrap();

    assert_eq!(
        candidates.len(),
        3,
        "get_candidates(10, 3) should return at most 3 URLs from one selected ListingSource"
    );
}

// ---------------------------------------------------------------------------
// get_candidates - random source-fair fetching caps ListingSources and URLs per ListingSource
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_cap_sources_and_urls_per_source_when_fetching_candidates() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let hot_listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, hot_listing_source_id, "fair-hot.example.com").await;
    for i in 0..10u32 {
        insert_product_url(
            &pool,
            hot_listing_source_id,
            &format!("https://fair-hot.example.com/p/{i}"),
        )
        .await;
    }

    for source_idx in 0..4u32 {
        let listing_source_id = ListingSourceId::new();
        let crawl_host = format!("fair-cold-{source_idx}.example.com");
        insert_listing_source(&pool, listing_source_id, &crawl_host).await;
        for url_idx in 0..4u32 {
            insert_product_url(
                &pool,
                listing_source_id,
                &format!("https://{crawl_host}/p/{url_idx}"),
            )
            .await;
        }
    }

    let candidates = service.get_candidates(3, 2, &[]).await.unwrap();

    let mut counts_by_host: HashMap<String, usize> = HashMap::new();
    for candidate in &candidates {
        let host = candidate
            .url
            .host_str()
            .expect("candidate URL should have host")
            .to_string();
        *counts_by_host.entry(host).or_default() += 1;
    }

    assert!(
        counts_by_host.len() <= 3,
        "candidate fetch should select at most 3 ListingSources"
    );
    assert!(
        counts_by_host.values().all(|count| *count <= 2),
        "candidate fetch should select at most 2 URLs per ListingSource"
    );
    assert!(
        candidates.len() <= 6,
        "candidate fetch should return at most N * M URLs"
    );
    assert!(
        counts_by_host
            .get("fair-hot.example.com")
            .copied()
            .unwrap_or(0)
            <= 2,
        "hot ListingSource must not fill the whole fetch"
    );
}

// ---------------------------------------------------------------------------
// get_candidates - selected ListingSource set changes across repeated calls
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_randomize_selected_sources_across_repeated_fetches() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    for i in 0..10u32 {
        let listing_source_id = ListingSourceId::new();
        let crawl_host = format!("random-source-{i}.example.com");
        insert_listing_source(&pool, listing_source_id, &crawl_host).await;
        insert_product_url(
            &pool,
            listing_source_id,
            &format!("https://{crawl_host}/p/1"),
        )
        .await;
    }

    let mut observed_sets: std::collections::HashSet<Vec<String>> =
        std::collections::HashSet::new();
    for _ in 0..20 {
        let candidates = service.get_candidates(3, 1, &[]).await.unwrap();
        let mut hosts: Vec<String> = candidates
            .iter()
            .filter_map(|candidate| candidate.url.host_str().map(str::to_string))
            .collect();
        hosts.sort();
        observed_sets.insert(hosts);
    }

    assert!(
        observed_sets.len() > 1,
        "random ListingSource selection should not return the same host set every time"
    );
}

// ---------------------------------------------------------------------------
// get_candidates - selected URLs inside one ListingSource are oldest first
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_order_urls_within_selected_source_by_oldest_work_first() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "fair-order.example.com").await;
    let never_url = "https://fair-order.example.com/p/never";
    let oldest_url = "https://fair-order.example.com/p/oldest";
    let newer_url = "https://fair-order.example.com/p/newer";
    insert_product_url(&pool, listing_source_id, newer_url).await;
    insert_product_url(&pool, listing_source_id, never_url).await;
    insert_product_url(&pool, listing_source_id, oldest_url).await;

    sqlx::query(
        "UPDATE listing_source_urls
         SET last_scraped = NOW() - INTERVAL '2 days',
             last_scraped_hash = 'newer'
         WHERE url = $1",
    )
    .bind(newer_url)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_scraped = NOW() - INTERVAL '5 days',
             last_scraped_hash = 'oldest'
         WHERE url = $1",
    )
    .bind(oldest_url)
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(1, 2, &[]).await.unwrap();
    let urls: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.url.as_str())
        .collect();

    assert_eq!(urls, vec![never_url, oldest_url]);
}

// ---------------------------------------------------------------------------
// get_candidates — pages waiting on a schema review yield their batch slots
// ---------------------------------------------------------------------------

/// Asks to record that `url` waits on `wait.review_id` with a retry that is already due.
async fn mark_due_schema_review_wait(
    service: &ScraperCandidateServiceImpl,
    listing_source_id: ListingSourceId,
    url: &str,
    wait: SchemaReviewWait,
) -> CrawlerUrlWriteOutcome {
    service
        .mark_waiting_on_schema_review(SchemaReviewWaitRequest {
            url_failure: fetch_failure_request(
                listing_source_id,
                url::Url::parse(url).unwrap(),
                "PendingSchemaReview",
                "blocked pending product schema review",
                None,
                time::OffsetDateTime::now_utc() - time::Duration::minutes(1),
                None,
            ),
            wait,
        })
        .await
        .unwrap()
}

/// Records that `url` waits on `wait.review_id` with a retry that is already due.
async fn record_due_schema_review_wait(
    service: &ScraperCandidateServiceImpl,
    listing_source_id: ListingSourceId,
    url: &str,
    wait: SchemaReviewWait,
) {
    let outcome = mark_due_schema_review_wait(service, listing_source_id, url, wait).await;
    assert_eq!(outcome, CrawlerUrlWriteOutcome::Applied);
}

/// Gives the ListingSource a cached schema and a pending product-schema review for its
/// failing pages to wait on.
async fn insert_schemas_and_pending_review(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
) -> CrawlerReviewId {
    insert_cached_product_schemas(
        pool,
        listing_source_id,
        serde_json::json!([{ "title": { "type": "text", "selector": "h1", "cardinality": "first" } }]),
    )
    .await;
    insert_pending_product_schema_review(pool, listing_source_id).await
}

async fn persisted_schema_review_wait(pool: &sqlx::PgPool, url: &str) -> (Option<uuid::Uuid>, i32) {
    sqlx::query_as(
        "SELECT schema_review_waited_on, schema_review_wait_streak \
         FROM listing_source_urls WHERE url = $1",
    )
    .bind(url)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_order_pages_waiting_on_a_schema_review_after_other_due_pages() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "waiting-order.example.com").await;
    let waiting_url = "https://waiting-order.example.com/p/a-waiting";
    let never_url = "https://waiting-order.example.com/p/never";
    let stale_url = "https://waiting-order.example.com/p/stale";
    for url in [waiting_url, never_url, stale_url] {
        insert_product_url(&pool, listing_source_id, url).await;
    }
    sqlx::query(
        "UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '5 days' WHERE url = $1",
    )
    .bind(stale_url)
    .execute(&pool)
    .await
    .unwrap();
    let review_id = insert_schemas_and_pending_review(&pool, listing_source_id).await;
    record_due_schema_review_wait(
        &service,
        listing_source_id,
        waiting_url,
        SchemaReviewWait {
            review_id,
            streak: 1,
        },
    )
    .await;

    let urls_in_batch = |limit| {
        let service = &service;
        async move {
            service
                .get_candidates(1, limit, &[])
                .await
                .unwrap()
                .into_iter()
                .map(|candidate| candidate.url.to_string())
                .collect::<Vec<_>>()
        }
    };

    assert_eq!(
        urls_in_batch(2).await,
        vec![never_url, stale_url],
        "a never-scraped page waiting on a review must not take a slot ahead of extractable pages"
    );
    assert_eq!(
        urls_in_batch(3).await,
        vec![never_url, stale_url, waiting_url]
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_keep_a_schema_review_wait_only_until_another_outcome_replaces_it() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "waiting-state.example.com").await;
    let url = "https://waiting-state.example.com/p/1";
    let parsed_url = url::Url::parse(url).unwrap();
    insert_product_url(&pool, listing_source_id, url).await;
    let wait = SchemaReviewWait {
        review_id: insert_schemas_and_pending_review(&pool, listing_source_id).await,
        streak: 3,
    };
    let candidate_wait = || async {
        let candidates = service.get_candidates(10, 10, &[]).await.unwrap();
        let [candidate] = candidates.as_slice() else {
            panic!("expected exactly one candidate");
        };
        candidate.schema_review_wait
    };

    record_due_schema_review_wait(&service, listing_source_id, url, wait).await;
    assert_eq!(candidate_wait().await, Some(wait));

    service
        .mark_scraper_failure(
            &listing_source_id,
            &parsed_url,
            "NormalizationError",
            "normalization failed",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        candidate_wait().await,
        None,
        "a later outcome ends the wait even when its write leaves the wait columns"
    );

    record_due_schema_review_wait(&service, listing_source_id, url, wait).await;
    service
        .mark_fetch_failure(
            &listing_source_id,
            &parsed_url,
            "HTTP_500",
            "server error",
            Some(500),
            time::OffsetDateTime::now_utc() - time::Duration::minutes(1),
            None,
        )
        .await
        .unwrap();
    assert_eq!(candidate_wait().await, None);
    assert_eq!(persisted_schema_review_wait(&pool, url).await, (None, 0));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_record_a_schema_review_wait_only_on_a_pending_review_of_its_source() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "waiting-guard.example.com").await;
    let url = "https://waiting-guard.example.com/p/1";
    insert_product_url(&pool, listing_source_id, url).await;
    let review_id = insert_schemas_and_pending_review(&pool, listing_source_id).await;
    let other_listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, other_listing_source_id, "other-guard.example.com").await;
    let other_source_review_id =
        insert_pending_product_schema_review(&pool, other_listing_source_id).await;
    let wait_on = |review_id| SchemaReviewWait {
        review_id,
        streak: 2,
    };

    for (review_id, case) in [
        (CrawlerReviewId::new(), "an unknown review"),
        (other_source_review_id, "another ListingSource's review"),
    ] {
        assert_eq!(
            mark_due_schema_review_wait(&service, listing_source_id, url, wait_on(review_id)).await,
            CrawlerUrlWriteOutcome::NoopStale,
            "a page must not wait on {case}"
        );
    }
    sqlx::query("UPDATE crawler_reviews SET status = 'REJECTED' WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        mark_due_schema_review_wait(&service, listing_source_id, url, wait_on(review_id)).await,
        CrawlerUrlWriteOutcome::NoopStale,
        "a page must not wait on a closed review"
    );

    assert_eq!(persisted_schema_review_wait(&pool, url).await, (None, 0));
    let (last_error_kind, failure_count): (Option<String>, i32) = sqlx::query_as(
        "SELECT last_error_kind, failure_count FROM listing_source_urls WHERE url = $1",
    )
    .bind(url)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((last_error_kind, failure_count), (None, 0));
}

// ---------------------------------------------------------------------------
// get_random_product_urls_for_schema_seed — a schema review wait is no failure
// ---------------------------------------------------------------------------

async fn url_failure_count(pool: &sqlx::PgPool, url: &str) -> i32 {
    sqlx::query_scalar("SELECT failure_count FROM listing_source_urls WHERE url = $1")
        .bind(url)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A page that only waited on a product-schema review keeps no failure and stays a schema seed,
/// however often it waited; a page whose fetch failed stays out of the sample, and a later wait
/// does not clear that failure.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_keep_a_page_that_only_waited_on_a_schema_review_as_a_seed() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "waiting-seed.example.com").await;
    let primary_url = "https://waiting-seed.example.com/p/primary";
    let waiting_url = "https://waiting-seed.example.com/p/waiting";
    let failing_url = "https://waiting-seed.example.com/p/failing";
    for url in [primary_url, waiting_url, failing_url] {
        insert_product_url(&pool, listing_source_id, url).await;
    }
    let review_id = insert_pending_product_schema_review(&pool, listing_source_id).await;
    let wait = |streak| SchemaReviewWait { review_id, streak };

    record_due_schema_review_wait(&service, listing_source_id, waiting_url, wait(1)).await;
    record_due_schema_review_wait(&service, listing_source_id, waiting_url, wait(2)).await;
    service
        .mark_fetch_failure(
            &listing_source_id,
            &url::Url::parse(failing_url).unwrap(),
            "HTTP_500",
            "server error",
            Some(500),
            time::OffsetDateTime::now_utc() - time::Duration::minutes(1),
            None,
        )
        .await
        .unwrap();
    record_due_schema_review_wait(&service, listing_source_id, failing_url, wait(1)).await;

    assert_eq!(url_failure_count(&pool, waiting_url).await, 0);
    assert_eq!(url_failure_count(&pool, failing_url).await, 1);
    let seeds: Vec<String> = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &[primary_url.to_owned()], 10)
        .await
        .unwrap()
        .into_iter()
        .map(|seed| seed.url.to_string())
        .collect();
    assert_eq!(seeds, vec![waiting_url]);
}

// ---------------------------------------------------------------------------
// get_candidates — the page-error run counts only page errors in a row
// ---------------------------------------------------------------------------

/// Each page error adds one to the URL's page-error run, any other failure or a scraper outcome
/// resets it, and failures of other kinds before the run never count into it (#2890).
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_page_error_streak_should_count_only_consecutive_page_errors() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "page-errors.example.com").await;
    let url = "https://page-errors.example.com/p/1";
    let parsed_url = url::Url::parse(url).unwrap();
    insert_product_url(&pool, listing_source_id, url).await;
    let fail = |error_kind: &'static str, status_code: Option<i32>| {
        let service = &service;
        let parsed_url = &parsed_url;
        async move {
            service
                .mark_fetch_failure(
                    &listing_source_id,
                    parsed_url,
                    error_kind,
                    "failed",
                    status_code,
                    time::OffsetDateTime::now_utc() - time::Duration::minutes(1),
                    None,
                )
                .await
                .unwrap()
        }
    };
    let candidate_streak = || async {
        let candidates = service.get_candidates(10, 10, &[]).await.unwrap();
        let [candidate] = candidates.as_slice() else {
            panic!("expected exactly one candidate");
        };
        candidate.page_error_streak
    };
    let stored_streak = || async {
        sqlx::query_scalar::<_, i32>(
            "SELECT page_error_streak FROM listing_source_urls WHERE url = $1",
        )
        .bind(url)
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    for _ in 0..9 {
        assert_eq!(fail("Timeout", None).await, CrawlerUrlWriteOutcome::Applied);
    }
    assert_eq!(candidate_streak().await, 0);
    fail("HTTP_500", Some(500)).await;
    assert_eq!(
        candidate_streak().await,
        1,
        "failures of other kinds before the run do not count into it"
    );
    fail("HTTP_502", Some(502)).await;
    fail("HTTP_400", Some(400)).await;
    assert_eq!(candidate_streak().await, 3);
    assert_eq!(url_failure_count(&pool, url).await, 12);

    fail("HTTP_503", Some(503)).await;
    assert_eq!(candidate_streak().await, 0);
    assert_eq!(
        stored_streak().await,
        0,
        "another failure kind resets the run"
    );

    fail("HTTP_500", Some(500)).await;
    assert_eq!(stored_streak().await, 1);
    service
        .mark_scraper_failure(
            &listing_source_id,
            &parsed_url,
            "NormalizationError",
            "normalization failed",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        stored_streak().await,
        0,
        "a page that answered ends the run"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — excluded active ListingSources are NOT returned
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_return_candidate_when_listing_source_is_excluded() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let blocked_listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        blocked_listing_source_id,
        "scraper-blocked.example.com",
    )
    .await;
    insert_product_url(
        &pool,
        blocked_listing_source_id,
        "https://scraper-blocked.example.com/p/1",
    )
    .await;

    let open_listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, open_listing_source_id, "scraper-open.example.com").await;
    insert_product_url(
        &pool,
        open_listing_source_id,
        "https://scraper-open.example.com/p/1",
    )
    .await;

    let excluded_listing_source_ids = vec![blocked_listing_source_id];
    let candidates = service
        .get_candidates(10, 100, &excluded_listing_source_ids)
        .await
        .unwrap();

    assert!(
        !candidates
            .iter()
            .any(|c| c.url.host_str() == Some("scraper-blocked.example.com")),
        "URL from excluded ListingSource should not be returned"
    );
    assert!(
        candidates
            .iter()
            .any(|c| c.url.host_str() == Some("scraper-open.example.com")),
        "URL from non-excluded ListingSource should still be returned"
    );
}

// ---------------------------------------------------------------------------
// mark_as_scraped — sets last_scraped and last_scraped_hash on the row
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_as_scraped_should_set_last_scraped_and_hash() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-mark.example.com").await;
    let url_str = "https://scraper-mark.example.com/p/1";
    insert_product_url(&pool, listing_source_id, url_str).await;

    let url = url::Url::parse(url_str).unwrap();
    let scraped_hash = "m".repeat(64);

    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            url.clone(),
            (
                scraped_hash.clone(),
                "schema-fingerprint",
                raw_input_hash(),
                "SKU-123",
                CrawlerDisposition::Active,
                None,
            ),
        ))
        .await
        .unwrap();

    let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT last_scraped_hash, last_scraped::text, last_source_listing_id FROM listing_source_urls WHERE url = $1",
    )
    .bind(url_str)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        row.0.as_deref(),
        Some(scraped_hash.as_str()),
        "last_scraped_hash should be updated"
    );
    assert!(row.1.is_some(), "last_scraped timestamp should be set");
    assert_eq!(row.2.as_deref(), Some("SKU-123"));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_normal_fallback_identity_should_remain_stable_across_completions() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "fallback-identity.example.com").await;
    let url = url::Url::parse("https://fallback-identity.example.com/product/foo").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;
    let fallback_identity =
        product_listing_normalization::normalize_source_listing_id_with_url_sha_fallback("", &url)
            .unwrap()
            .to_string();

    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "first-hash",
                    "schema",
                    vec![1; 32],
                    fallback_identity.clone(),
                    CrawlerDisposition::Active,
                    None,
                ),
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "second-hash",
                    "schema",
                    vec![2; 32],
                    fallback_identity.clone(),
                    CrawlerDisposition::Active,
                    Some(vec![1; 32]),
                ),
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let stored: Option<String> = sqlx::query_scalar(
        "SELECT last_source_listing_id FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(fallback_identity.as_str()));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn redirected_scrape_should_move_unknown_destination_and_preserve_identity_metadata() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "redirect-move.example.com").await;
    let original_url = url::Url::parse("https://redirect-move.example.com/old").unwrap();
    let effective_url = url::Url::parse("https://redirect-move.example.com/new").unwrap();
    insert_product_url(&pool, listing_source_id, original_url.as_str()).await;

    assert_eq!(
        service
            .move_redirected_url(RedirectUrlMove {
                listing_source_id,
                original_url: original_url.clone(),
                effective_url: effective_url.clone(),
                hash: "redirect-hash".to_owned(),
                schema_fingerprint: "redirect-schema".to_owned(),
                raw_input_sha256: vec![4; 32],
                source_listing_id: "stable-source-id".to_owned(),
                title: "Captured title".to_owned(),
                disposition: CrawlerDisposition::Active,
                expected_last_captured_raw_input_sha256: None,
            })
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT url, raw_source_record_key, last_source_listing_id \
         FROM listing_source_urls WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, effective_url.as_str());
    assert_eq!(row.1, original_url.as_str());
    assert_eq!(row.2.as_deref(), Some("stable-source-id"));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn redirected_scrape_should_reject_known_destination_without_mutating_rows() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "redirect-known-destination.example.com",
    )
    .await;
    let original_url =
        url::Url::parse("https://redirect-known-destination.example.com/old").unwrap();
    let effective_url =
        url::Url::parse("https://redirect-known-destination.example.com/new").unwrap();
    insert_product_url(&pool, listing_source_id, original_url.as_str()).await;
    insert_product_url(&pool, listing_source_id, effective_url.as_str()).await;

    assert_eq!(
        service
            .redirect_destination(&listing_source_id, &effective_url)
            .await
            .unwrap(),
        RedirectDestination::ProductOfListingSource
    );
    let error = service
        .move_redirected_url(RedirectUrlMove {
            listing_source_id,
            original_url: original_url.clone(),
            effective_url: effective_url.clone(),
            hash: "redirect-hash".to_owned(),
            schema_fingerprint: "redirect-schema".to_owned(),
            raw_input_sha256: vec![5; 32],
            source_listing_id: "stable-source-id".to_owned(),
            title: "Captured title".to_owned(),
            disposition: CrawlerDisposition::Active,
            expected_last_captured_raw_input_sha256: None,
        })
        .await
        .expect_err("known destination must be deferred to reconciliation");
    assert!(
        error
            .to_string()
            .contains("redirect destination already known")
    );

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT url, raw_source_record_key FROM listing_source_urls \
         WHERE listing_source_id = $1 ORDER BY url",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row.0 == original_url.as_str()));
    assert!(rows.iter().any(|row| row.0 == effective_url.as_str()));
    assert!(rows.iter().any(|row| row.1 == original_url.as_str()));
    assert!(rows.iter().any(|row| row.1 == effective_url.as_str()));
}

/// Only a product URL of the redirected URL's own ListingSource that is not itself retired makes
/// the redirected URL a duplicate (`docs/crawler/networking.md#duplicate-targets`). A page stored
/// by another ListingSource or as a non-product row is no product page of the ListingSource, and
/// rows that disagree settle nothing. Rows are inserted directly, as any
/// persisted row on the redirect's host must be judged by its owner and class.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn redirect_destination_should_count_only_a_live_product_url_of_the_same_listing_source() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let host = "redirect-scope.example.com";
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, host).await;
    let other_listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        other_listing_source_id,
        "other-redirect-scope.example.com",
    )
    .await;
    let store = |listing_source_id: ListingSourceId,
                 path: &str,
                 class: UrlClass,
                 disposition: CrawlerDisposition| {
        let pool = pool.clone();
        let url = format!("https://{host}{path}");
        async move {
            sqlx::query(
                "INSERT INTO listing_source_urls \
                     (listing_source_id, url, raw_source_record_key, url_class, \
                      crawler_disposition, created, updated) \
                 VALUES ($1, $2, $2, $3, $4, NOW(), NOW())",
            )
            .bind(listing_source_id.as_uuid())
            .bind(&url)
            .bind(class.as_str())
            .bind(disposition.as_str())
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    use CrawlerDisposition::{Active, DormantSold, Duplicate};
    use UrlClass::{Category, Other, ProductListing};
    store(listing_source_id, "/products/live", ProductListing, Active).await;
    store(
        listing_source_id,
        "/products/sold",
        ProductListing,
        DormantSold,
    )
    .await;
    store(
        listing_source_id,
        "/products/retired",
        ProductListing,
        Duplicate,
    )
    .await;
    // A page stored before the ListingSource had a product pattern keeps the class it was stored with
    // when a later crawl retires it as a duplicate of its canonical URL.
    store(
        listing_source_id,
        "/collections/a/products/retired",
        Other,
        Duplicate,
    )
    .await;
    store(listing_source_id, "/category/chairs", Category, Active).await;
    store(
        other_listing_source_id,
        "/products/elsewhere",
        ProductListing,
        Active,
    )
    .await;
    // Both trailing-slash spellings of one page, one a product URL and one a category row.
    store(listing_source_id, "/products/mixed", ProductListing, Active).await;
    store(listing_source_id, "/products/mixed/", Category, Active).await;

    for (path, expected) in [
        ("/products/unknown", RedirectDestination::Unknown),
        (
            "/products/live",
            RedirectDestination::ProductOfListingSource,
        ),
        (
            "/products/live/",
            RedirectDestination::ProductOfListingSource,
        ),
        (
            "/products/sold",
            RedirectDestination::ProductOfListingSource,
        ),
        (
            "/category/chairs",
            RedirectDestination::NotAProductOfListingSource,
        ),
        (
            "/products/elsewhere",
            RedirectDestination::NotAProductOfListingSource,
        ),
        ("/products/retired", RedirectDestination::Unsettled),
        (
            "/collections/a/products/retired",
            RedirectDestination::Unsettled,
        ),
        ("/products/mixed", RedirectDestination::Unsettled),
    ] {
        let destination = url::Url::parse(&format!("https://{host}{path}")).unwrap();
        assert_eq!(
            service
                .redirect_destination(&listing_source_id, &destination)
                .await
                .unwrap(),
            expected,
            "{destination}"
        );
    }
}

/// A page with and without its trailing slash is one page, so a redirect to a page stored under
/// the other spelling is a known destination as well: moving the row there would give the page a
/// second row.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn redirected_scrape_should_reject_destination_known_under_other_trailing_slash_spelling() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    for (index, (stored_destination, effective_path)) in [("/new", "/new/"), ("/new/", "/new")]
        .into_iter()
        .enumerate()
    {
        let listing_source_id = ListingSourceId::new();
        let host = format!("redirect-slash-{index}.example.com");
        insert_listing_source(&pool, listing_source_id, &host).await;
        let original_url = format!("https://{host}/old");
        let stored_destination = format!("https://{host}{stored_destination}");
        let effective_url = url::Url::parse(&format!("https://{host}{effective_path}")).unwrap();
        insert_product_url(&pool, listing_source_id, &original_url).await;
        insert_product_url(&pool, listing_source_id, &stored_destination).await;

        assert_eq!(
            service
                .redirect_destination(&listing_source_id, &effective_url)
                .await
                .unwrap(),
            RedirectDestination::ProductOfListingSource,
            "{effective_url}"
        );
        let error = service
            .move_redirected_url(RedirectUrlMove {
                listing_source_id,
                original_url: url::Url::parse(&original_url).unwrap(),
                effective_url: effective_url.clone(),
                hash: "redirect-hash".to_owned(),
                schema_fingerprint: "redirect-schema".to_owned(),
                raw_input_sha256: vec![6; 32],
                source_listing_id: "stable-source-id".to_owned(),
                title: "Captured title".to_owned(),
                disposition: CrawlerDisposition::Active,
                expected_last_captured_raw_input_sha256: None,
            })
            .await
            .expect_err("a destination known under its other spelling must be deferred");
        assert!(
            error
                .to_string()
                .contains("redirect destination already known"),
            "{effective_url}"
        );

        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT url FROM listing_source_urls WHERE listing_source_id = $1 ORDER BY url",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            rows,
            vec![stored_destination, original_url],
            "{effective_url}"
        );
    }
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn redirected_scrape_should_reject_identity_conflict_and_stale_fence() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "redirect-identity-fence.example.com",
    )
    .await;
    let original_url = url::Url::parse("https://redirect-identity-fence.example.com/old").unwrap();
    let effective_url = url::Url::parse("https://redirect-identity-fence.example.com/new").unwrap();
    insert_product_url(&pool, listing_source_id, original_url.as_str()).await;

    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            original_url.clone(),
            (
                "initial-hash",
                "initial-schema",
                vec![6; 32],
                "SKU-1",
                CrawlerDisposition::Active,
                None,
            ),
        ))
        .await
        .unwrap();

    let conflict = service
        .move_redirected_url(RedirectUrlMove {
            listing_source_id,
            original_url: original_url.clone(),
            effective_url: effective_url.clone(),
            hash: "redirect-hash".to_owned(),
            schema_fingerprint: "redirect-schema".to_owned(),
            raw_input_sha256: vec![7; 32],
            source_listing_id: "SKU-2".to_owned(),
            title: "Captured title".to_owned(),
            disposition: CrawlerDisposition::Active,
            expected_last_captured_raw_input_sha256: Some(vec![6; 32]),
        })
        .await
        .expect_err("identity changes must be rejected");
    assert!(conflict.to_string().contains("identity conflicts"));

    assert_eq!(
        service
            .move_redirected_url(RedirectUrlMove {
                listing_source_id,
                original_url,
                effective_url,
                hash: "stale-hash".to_owned(),
                schema_fingerprint: "stale-schema".to_owned(),
                raw_input_sha256: vec![8; 32],
                source_listing_id: "SKU-1".to_owned(),
                title: "Captured title".to_owned(),
                disposition: CrawlerDisposition::Active,
                expected_last_captured_raw_input_sha256: Some(vec![9; 32]),
            })
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_completion_should_not_overwrite_newer_dormant_scrape_metadata() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-competing-completion.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-competing-completion.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let dormant_raw_input_hash = vec![9; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "dormant-hash",
                    "dormant-schema",
                    dormant_raw_input_hash.clone(),
                    "SKU-123",
                    CrawlerDisposition::DormantSold,
                    None,
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "delayed-active-hash",
                    "delayed-active-schema",
                    vec![8; 32],
                    "SKU-123",
                    CrawlerDisposition::Active,
                    None,
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );

    let row: (String, Option<String>, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT crawler_disposition, last_scraped_hash, last_scraped_schema_fingerprint, \
         last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, CrawlerDisposition::DormantSold.as_str());
    assert_eq!(row.1.as_deref(), Some("dormant-hash"));
    assert_eq!(row.2.as_deref(), Some("dormant-schema"));
    assert_eq!(row.3, Some(dormant_raw_input_hash));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_newer_active_completion_should_fence_delayed_active_completion_and_touch() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-active-fence.example.com").await;
    let url = url::Url::parse("https://scraper-active-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let observed_raw_input_sha256: Option<&[u8]> = None;
    let newer_raw_input_sha256 = vec![9; 32];

    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "newer-page-hash",
                    "newer-schema-fingerprint",
                    newer_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    observed_raw_input_sha256.map(ToOwned::to_owned),
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "delayed-page-hash",
                    "delayed-schema-fingerprint",
                    vec![8; 32],
                    "SKU-123",
                    CrawlerDisposition::Active,
                    observed_raw_input_sha256.map(ToOwned::to_owned),
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );
    assert_eq!(
        service
            .touch_scraped(
                &listing_source_id,
                &url,
                "delayed-page-hash",
                "delayed-schema-fingerprint",
                observed_raw_input_sha256,
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );

    let row: (String, Option<String>, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT crawler_disposition, last_scraped_hash, last_scraped_schema_fingerprint, \
         last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, CrawlerDisposition::Active.as_str());
    assert_eq!(row.1.as_deref(), Some("newer-page-hash"));
    assert_eq!(row.2.as_deref(), Some("newer-schema-fingerprint"));
    assert_eq!(row.3, Some(newer_raw_input_sha256));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_newer_active_completion_should_fence_delayed_sold_completion() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-sold-fence.example.com").await;
    let url = url::Url::parse("https://scraper-sold-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let observed_raw_input_sha256 = vec![1; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "observed-page-hash",
                    "observed-schema-fingerprint",
                    observed_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    None,
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let newer_raw_input_sha256 = vec![2; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "newer-page-hash",
                    "newer-schema-fingerprint",
                    newer_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    Some(observed_raw_input_sha256.clone()),
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "delayed-sold-page-hash",
                    "delayed-sold-schema-fingerprint",
                    vec![3; 32],
                    "SKU-123",
                    CrawlerDisposition::DormantSold,
                    Some(observed_raw_input_sha256.clone()),
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );

    let row: (String, Option<String>, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT crawler_disposition, last_scraped_hash, last_scraped_schema_fingerprint, \
         last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, CrawlerDisposition::Active.as_str());
    assert_eq!(row.1.as_deref(), Some("newer-page-hash"));
    assert_eq!(row.2.as_deref(), Some("newer-schema-fingerprint"));
    assert_eq!(row.3, Some(newer_raw_input_sha256));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_newer_active_completion_should_fence_delayed_disposition_change() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-removal-fence.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-removal-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let observed_raw_input_sha256 = vec![4; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "observed-page-hash",
                    "observed-schema-fingerprint",
                    observed_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    None,
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let newer_raw_input_sha256 = vec![5; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "newer-page-hash",
                    "newer-schema-fingerprint",
                    newer_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    Some(observed_raw_input_sha256.clone()),
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        service
            .set_disposition(
                &listing_source_id,
                &url,
                CrawlerDisposition::DormantSold,
                Some(observed_raw_input_sha256.as_slice()),
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale
    );

    let row: (String, Option<String>, Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT crawler_disposition, last_scraped_hash, last_scraped_schema_fingerprint, \
         last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.0, CrawlerDisposition::Active.as_str());
    assert_eq!(row.1.as_deref(), Some("newer-page-hash"));
    assert_eq!(row.2.as_deref(), Some("newer-schema-fingerprint"));
    assert_eq!(row.3, Some(newer_raw_input_sha256));
}

#[derive(sqlx::FromRow)]
struct DelayedObserverUrlState {
    url_class: String,
    crawler_disposition: String,
    last_scraped_hash: Option<String>,
    last_captured_raw_input_sha256: Option<Vec<u8>>,
    failure_count: i64,
    last_error_kind: Option<String>,
    last_error_message: Option<String>,
    no_next_retry_at: bool,
    no_last_status_code: bool,
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_fence_delayed_observer_local_writes_when_newer_active_capture_commits() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-delayed-observer-fence.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-delayed-observer-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let observed_raw_input_sha256 = vec![6; 32];
    assert_eq!(
        service
            .mark_as_scraped(scrape_completion(
                listing_source_id,
                url.clone(),
                (
                    "observed-page-hash",
                    "observed-schema-fingerprint",
                    observed_raw_input_sha256.clone(),
                    "SKU-123",
                    CrawlerDisposition::Active,
                    None,
                )
            ))
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let start = Arc::new(Barrier::new(2));
    let newer_committed = Arc::new(Barrier::new(2));
    let newer_raw_input_sha256 = vec![7; 32];

    let newer_task = {
        let service = ScraperCandidateServiceImpl::new(pool.clone());
        let newer_listing_source_id = listing_source_id;
        let url = url.clone();
        let observed_raw_input_sha256 = observed_raw_input_sha256.clone();
        let newer_raw_input_sha256 = newer_raw_input_sha256.clone();
        let start = Arc::clone(&start);
        let newer_committed = Arc::clone(&newer_committed);

        test_api::spawn_owned(async move {
            start.wait().await;
            assert_eq!(
                service
                    .mark_as_scraped(scrape_completion(
                        newer_listing_source_id,
                        url.clone(),
                        (
                            "newer-page-hash",
                            "newer-schema-fingerprint",
                            newer_raw_input_sha256.clone(),
                            "SKU-123",
                            CrawlerDisposition::Active,
                            Some(observed_raw_input_sha256.clone()),
                        )
                    ))
                    .await
                    .unwrap(),
                CrawlerUrlWriteOutcome::Applied
            );
            newer_committed.wait().await;
        })
    };

    let delayed_task = {
        let service = ScraperCandidateServiceImpl::new(pool.clone());
        let delayed_listing_source_id = listing_source_id;
        let url = url.clone();
        let observed_raw_input_sha256 = observed_raw_input_sha256.clone();
        let start = Arc::clone(&start);
        let newer_committed = Arc::clone(&newer_committed);

        test_api::spawn_owned(async move {
            start.wait().await;
            newer_committed.wait().await;

            assert_eq!(
                service
                    .set_class(
                        &delayed_listing_source_id,
                        &url,
                        UrlClass::Other,
                        Some(observed_raw_input_sha256.as_slice()),
                    )
                    .await
                    .unwrap(),
                CrawlerUrlWriteOutcome::NoopStale
            );
            assert_eq!(
                service
                    .mark_fetch_failure(
                        &delayed_listing_source_id,
                        &url,
                        "DelayedFetchFailure",
                        "delayed observer",
                        Some(500),
                        time::OffsetDateTime::now_utc() + time::Duration::hours(1),
                        Some(observed_raw_input_sha256.as_slice()),
                    )
                    .await
                    .unwrap(),
                CrawlerUrlWriteOutcome::NoopStale
            );
            assert_eq!(
                service
                    .mark_scraper_failure(
                        &delayed_listing_source_id,
                        &url,
                        "DelayedScraperFailure",
                        "delayed observer",
                        Some(observed_raw_input_sha256.as_slice()),
                    )
                    .await
                    .unwrap(),
                CrawlerUrlWriteOutcome::NoopStale
            );
            assert_eq!(
                service
                    .set_disposition(
                        &delayed_listing_source_id,
                        &url,
                        CrawlerDisposition::DormantSold,
                        Some(observed_raw_input_sha256.as_slice()),
                    )
                    .await
                    .unwrap(),
                CrawlerUrlWriteOutcome::NoopStale
            );
        })
    };

    newer_task.await.unwrap();
    delayed_task.await.unwrap();

    let row = sqlx::query_as::<_, DelayedObserverUrlState>(
        "SELECT url_class, crawler_disposition, last_scraped_hash, \
         last_captured_raw_input_sha256, failure_count::bigint AS failure_count, \
         last_error_kind, last_error_message, next_retry_at IS NULL AS no_next_retry_at, \
         last_status_code IS NULL AS no_last_status_code \
         FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(row.url_class, UrlClass::ProductListing.as_str());
    assert_eq!(row.crawler_disposition, CrawlerDisposition::Active.as_str());
    assert_eq!(row.last_scraped_hash.as_deref(), Some("newer-page-hash"));
    assert_eq!(
        row.last_captured_raw_input_sha256,
        Some(newer_raw_input_sha256)
    );
    assert_eq!(row.failure_count, 0);
    assert!(row.last_error_kind.is_none());
    assert!(row.last_error_message.is_none());
    assert!(row.no_next_retry_at);
    assert!(row.no_last_status_code);
}

// ---------------------------------------------------------------------------
// mark_as_scraped — after marking, the URL is excluded from get_candidates
//                   (recently scraped)
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_mark_as_scraped_should_exclude_url_from_subsequent_get_candidates() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "scraper-roundtrip.example.com").await;
    let url_str = "https://scraper-roundtrip.example.com/p/1";
    let hash = "n".repeat(64);
    insert_product_url(&pool, listing_source_id, url_str).await;

    // Confirm it is returned before marking
    let before = service.get_candidates(10, 100, &[]).await.unwrap();
    assert!(
        before.iter().any(|c| c.url.as_str() == url_str),
        "URL should appear before mark_as_scraped"
    );

    let url = url::Url::parse(url_str).unwrap();
    service
        .mark_as_scraped(scrape_completion(
            listing_source_id,
            url,
            (
                hash,
                "schema-fingerprint",
                raw_input_hash(),
                "SKU-123",
                CrawlerDisposition::Active,
                None,
            ),
        ))
        .await
        .unwrap();

    // After marking with NOW() timestamp it should no longer appear
    let after = service.get_candidates(10, 100, &[]).await.unwrap();
    assert!(
        !after.iter().any(|c| c.url.as_str() == url_str),
        "URL should not appear after mark_as_scraped"
    );
}

// ---------------------------------------------------------------------------
// get_random_product_urls_for_schema_seed — same ListingSource, product class, and presence
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_seed_urls_should_include_same_listing_source_sold_product_urls() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let seed_listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, seed_listing_source_id, "seed-filters.example.com").await;
    let repo = UrlMetadataRepositoryImpl::new(pool.clone());

    let current_url = url::Url::parse("https://seed-filters.example.com/p/current").unwrap();
    repo.upsert_link(
        &seed_listing_source_id,
        &current_url,
        &UrlClass::ProductListing,
    )
    .await
    .unwrap();

    let eligible_url = url::Url::parse("https://seed-filters.example.com/p/eligible").unwrap();
    repo.upsert_link(
        &seed_listing_source_id,
        &eligible_url,
        &UrlClass::ProductListing,
    )
    .await
    .unwrap();

    let sold_url = url::Url::parse("https://seed-filters.example.com/p/sold").unwrap();
    repo.upsert_link(
        &seed_listing_source_id,
        &sold_url,
        &UrlClass::ProductListing,
    )
    .await
    .unwrap();
    repo.set_disposition(
        &seed_listing_source_id,
        &sold_url,
        CrawlerDisposition::DormantSold,
    )
    .await
    .unwrap();

    let withdrawn_url = url::Url::parse("https://seed-filters.example.com/p/withdrawn").unwrap();
    repo.upsert_link(
        &seed_listing_source_id,
        &withdrawn_url,
        &UrlClass::ProductListing,
    )
    .await
    .unwrap();
    repo.set_disposition(
        &seed_listing_source_id,
        &withdrawn_url,
        CrawlerDisposition::DormantSold,
    )
    .await
    .unwrap();

    let category_url =
        url::Url::parse("https://seed-filters.example.com/category/whatever").unwrap();
    repo.upsert_link(&seed_listing_source_id, &category_url, &UrlClass::Category)
        .await
        .unwrap();

    let other_listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        other_listing_source_id,
        "seed-other-source.example.com",
    )
    .await;
    let other_listing_source_url =
        url::Url::parse("https://seed-other-source.example.com/p/other").unwrap();
    repo.upsert_link(
        &other_listing_source_id,
        &other_listing_source_url,
        &UrlClass::ProductListing,
    )
    .await
    .unwrap();

    let sampled = service
        .get_random_product_urls_for_schema_seed(
            &seed_listing_source_id,
            &[current_url.to_string()],
            20,
        )
        .await
        .unwrap();

    assert!(
        sampled
            .iter()
            .any(|candidate| candidate.url == eligible_url),
        "eligible product URL from same ListingSource should be included"
    );
    assert!(
        sampled.iter().all(|candidate| candidate.url != current_url),
        "current URL must be excluded"
    );
    assert!(
        sampled.iter().any(|candidate| candidate.url == sold_url),
        "sold product URLs must be included for schema seeding"
    );
    assert!(
        sampled
            .iter()
            .any(|candidate| candidate.url == withdrawn_url),
        "a second sold product URL must be included for schema seeding"
    );
    assert!(
        sampled
            .iter()
            .all(|candidate| candidate.url != category_url),
        "non-product URLs must be excluded"
    );
    assert!(
        sampled
            .iter()
            .all(|candidate| candidate.url != other_listing_source_url),
        "URLs from other listing_sources must be excluded"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_seed_urls_should_not_bypass_an_open_scrape_circuit() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "seed-circuit.example.com").await;

    let current = "https://seed-circuit.example.com/p/current";
    let seed = "https://seed-circuit.example.com/p/seed";
    insert_product_url(&pool, listing_source_id, current).await;
    insert_product_url(&pool, listing_source_id, seed).await;
    let excluded = [url::Url::parse(current).unwrap().to_string()];

    let sampled = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &excluded, 10)
        .await
        .unwrap();
    assert_eq!(sampled.len(), 1);
    assert_eq!(sampled[0].url, url::Url::parse(seed).unwrap());

    sqlx::query(
        "UPDATE listing_sources
         SET scrape_failure_streak = 1,
             last_scrape_error_kind = 'HTTP_429',
             next_scrape_at = NOW() + INTERVAL '15 minutes'
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let sampled = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &excluded, 10)
        .await
        .unwrap();
    assert!(
        sampled.is_empty(),
        "an open scrape circuit must not be bypassed through schema seeding"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_seed_failure_updates_seed_url_without_poisoning_primary_url() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "seed-failure-provenance.example.com",
    )
    .await;
    let primary_url =
        url::Url::parse("https://seed-failure-provenance.example.com/p/primary").unwrap();
    let seed_url = url::Url::parse("https://seed-failure-provenance.example.com/p/seed").unwrap();
    insert_product_url(&pool, listing_source_id, primary_url.as_str()).await;
    insert_product_url(&pool, listing_source_id, seed_url.as_str()).await;

    let seed_hash = vec![2; 32];
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("01".repeat(32))
    .bind(primary_url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("02".repeat(32))
    .bind(seed_url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let selected_seed = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &[primary_url.to_string()], 1)
        .await
        .unwrap();
    assert_eq!(selected_seed.len(), 1);
    assert_eq!(selected_seed[0].url, seed_url);
    assert_eq!(
        selected_seed[0].expected_last_captured_raw_input_sha256,
        Some(seed_hash.clone())
    );

    let primary_next_retry_at = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_error_kind = 'PRIMARY_SENTINEL',
             last_status_code = 418,
             next_retry_at = $2
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind(primary_next_retry_at)
    .bind(primary_url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    // Simulate another worker changing P after the primary scrape. The seed
    // request carries S's own fence, so this stale P state is irrelevant.
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("09".repeat(32))
    .bind(primary_url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let expected_scrape_health = ScrapeHealthSnapshot {
        scrape_failure_streak: 0,
        last_scrape_error_kind: None,
        next_scrape_at: None,
    };
    let next_retry_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                seed_url.clone(),
                "HTTP_429",
                "schema seed rate limited",
                Some(429),
                next_retry_at,
                Some(seed_hash),
            ),
            expected_scrape_health,
            DomainFailureKind::Http429,
            Some(429),
            next_retry_at,
        ))
        .await
        .unwrap();
    assert_eq!(outcome.url_failure, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);

    let primary: (i32, String, i32, time::OffsetDateTime) = sqlx::query_as(
        "SELECT failure_count, last_error_kind, last_status_code, next_retry_at
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(primary_url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(primary.0, 0);
    assert_eq!(primary.1, "PRIMARY_SENTINEL");
    assert_eq!(primary.2, 418);
    assert!(
        (primary.3 - primary_next_retry_at)
            .whole_microseconds()
            .abs()
            <= 1
    );

    let seed: (i32, String, i32, time::OffsetDateTime) = sqlx::query_as(
        "SELECT failure_count, last_error_kind, last_status_code, next_retry_at
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(seed_url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seed.0, 1);
    assert_eq!(seed.1, "HTTP_429");
    assert_eq!(seed.2, 429);
    assert!((seed.3 - next_retry_at).whole_microseconds().abs() <= 1);

    let source_row: (i32, String, i32, time::OffsetDateTime) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, last_scrape_status_code, next_scrape_at
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_row.0, 1);
    assert_eq!(source_row.1, "HTTP_429");
    assert_eq!(source_row.2, 429);
    assert!(source_row.3 > time::OffsetDateTime::now_utc());

    let sampled = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &[primary_url.to_string()], 10)
        .await
        .unwrap();
    assert!(
        sampled.is_empty(),
        "the failed seed must not be selected during the active retry cooldown"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_primary_429_with_non_null_fence_updates_url_and_opens_scrape_circuit() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "primary-failure-fence.example.com",
    )
    .await;
    let url = url::Url::parse("https://primary-failure-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let expected_fence = raw_input_hash();
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("07".repeat(32))
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let next_retry_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "primary rate limited",
                Some(429),
                next_retry_at,
                Some(expected_fence),
            ),
            ScrapeHealthSnapshot {
                scrape_failure_streak: 0,
                last_scrape_error_kind: None,
                next_scrape_at: None,
            },
            DomainFailureKind::Http429,
            Some(429),
            next_retry_at,
        ))
        .await
        .unwrap();

    assert_eq!(outcome.url_failure, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.persisted_failure_streak, Some(1));

    let url_row: (i32, String, i32, time::OffsetDateTime) = sqlx::query_as(
        "SELECT failure_count, last_error_kind, last_status_code, next_retry_at
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(url_row.0, 1);
    assert_eq!(url_row.1, "HTTP_429");
    assert_eq!(url_row.2, 429);
    assert!((url_row.3 - next_retry_at).whole_microseconds().abs() <= 1);

    let source_row: (i32, String, i32, time::OffsetDateTime) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, last_scrape_status_code, next_scrape_at
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_row.0, 1);
    assert_eq!(source_row.1, "HTTP_429");
    assert_eq!(source_row.2, 429);
    assert!(source_row.3 > time::OffsetDateTime::now_utc());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_recovery_probe_429_with_non_null_fence_reopens_scrape_circuit() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "recovery-probe-fence.example.com").await;
    let first_url = "https://recovery-probe-fence.example.com/p/1";
    let second_url = "https://recovery-probe-fence.example.com/p/2";
    insert_product_url(&pool, listing_source_id, first_url).await;
    insert_product_url(&pool, listing_source_id, second_url).await;

    let expired_at = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
    sqlx::query(
        "UPDATE listing_sources
         SET scrape_failure_streak = 1,
             last_scrape_error_kind = 'HTTP_429',
             last_scrape_status_code = 429,
             next_scrape_at = $2
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(expired_at)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind("08".repeat(32))
    .execute(&pool)
    .await
    .unwrap();

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    assert_eq!(candidates.len(), 1);
    let probe = &candidates[0];
    assert!(probe.is_circuit_probe);
    assert!(probe.last_captured_raw_input_sha256.is_some());

    let probe_url = probe.url.clone();
    let probe_fence = probe.last_captured_raw_input_sha256.clone();
    let next_retry_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    let next_scrape_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(20);
    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                probe_url.clone(),
                "HTTP_429",
                "recovery probe rate limited",
                Some(429),
                next_retry_at,
                probe_fence,
            ),
            ScrapeHealthSnapshot {
                scrape_failure_streak: 1,
                last_scrape_error_kind: Some("HTTP_429".to_owned()),
                next_scrape_at: Some(expired_at),
            },
            DomainFailureKind::Http429,
            Some(429),
            next_scrape_at,
        ))
        .await
        .unwrap();

    assert_eq!(outcome.url_failure, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.persisted_failure_streak, Some(2));

    let source_row: (i32, String, time::OffsetDateTime) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, next_scrape_at
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_row.0, 2);
    assert_eq!(source_row.1, "HTTP_429");
    assert!(source_row.2 > expired_at);
    assert!(source_row.2 > time::OffsetDateTime::now_utc());

    let candidates_after_failure = service.get_candidates(10, 100, &[]).await.unwrap();
    assert!(
        candidates_after_failure.is_empty(),
        "the reopened scrape circuit must not immediately select the failed probe"
    );
}

/// A slow-host failure recorded as evidence (a streak without a cooldown)
/// leaves the ListingSource open: the next pass selects its full batch, not a probe.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_recorded_scrape_failure_keeps_the_full_batch_eligible() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "recorded-evidence.example.com").await;
    let first_url = "https://recorded-evidence.example.com/p/1";
    let second_url = "https://recorded-evidence.example.com/p/2";
    insert_product_url(&pool, listing_source_id, first_url).await;
    insert_product_url(&pool, listing_source_id, second_url).await;

    let next_retry_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(5);
    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(ScrapeCircuitOpenRequest {
            url_failure: fetch_failure_request(
                listing_source_id,
                url::Url::parse(first_url).unwrap(),
                "Timeout",
                "timed out",
                None,
                next_retry_at,
                None,
            ),
            expected_scrape_health: ScrapeHealthSnapshot::default(),
            scrape_error_kind: DomainFailureKind::Timeout,
            scrape_status_code: None,
            next_scrape_at: None,
        })
        .await
        .unwrap();
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.persisted_failure_streak, Some(1));

    let candidates = service.get_candidates(10, 100, &[]).await.unwrap();
    assert_eq!(candidates.len(), 1, "the failed URL waits for its retry");
    let candidate = &candidates[0];
    assert_eq!(candidate.url.as_str(), second_url);
    assert!(!candidate.is_circuit_probe);
    assert_eq!(
        candidate.scrape_health,
        ScrapeHealthSnapshot {
            scrape_failure_streak: 1,
            last_scrape_error_kind: Some("TIMEOUT".to_owned()),
            next_scrape_at: None,
        }
    );

    let closed = service
        .close_scrape_circuit(&listing_source_id, &candidate.scrape_health)
        .await
        .unwrap();
    assert_eq!(closed, CrawlerUrlWriteOutcome::Applied);
}

/// `scrape_failing_since` dates the current same-kind streak: opening from a closed circuit
/// sets it, a further failure of the same kind keeps it, another kind restarts it and closing the
/// circuit clears it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_scrape_circuit_dates_the_current_failure_streak() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "failing-since.example.com").await;
    let url = "https://failing-since.example.com/p/1";
    insert_product_url(&pool, listing_source_id, url).await;

    async fn failing_since(
        pool: &sqlx::PgPool,
        listing_source_id: ListingSourceId,
    ) -> Option<time::OffsetDateTime> {
        sqlx::query_scalar(
            "SELECT scrape_failing_since FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
    }
    async fn open(
        service: &ScraperCandidateServiceImpl,
        listing_source_id: ListingSourceId,
        url: &str,
        health: ScrapeHealthSnapshot,
        kind: DomainFailureKind,
    ) -> ScrapeHealthSnapshot {
        let next_scrape_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(30);
        let outcome = service
            .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
                fetch_failure_request(
                    listing_source_id,
                    url::Url::parse(url).unwrap(),
                    "Failure",
                    "failed",
                    None,
                    time::OffsetDateTime::now_utc() + time::Duration::minutes(5),
                    None,
                ),
                health,
                kind,
                None,
                next_scrape_at,
            ))
            .await
            .unwrap();
        assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
        ScrapeHealthSnapshot {
            scrape_failure_streak: i32::try_from(outcome.persisted_failure_streak.unwrap())
                .unwrap(),
            last_scrape_error_kind: Some(kind.as_str().to_owned()),
            next_scrape_at: Some(next_scrape_at),
        }
    }

    assert_eq!(failing_since(&pool, listing_source_id).await, None);

    let health = open(
        &service,
        listing_source_id,
        url,
        ScrapeHealthSnapshot::default(),
        DomainFailureKind::Timeout,
    )
    .await;
    let began = failing_since(&pool, listing_source_id)
        .await
        .expect("opening from streak 0 dates the streak");
    assert!(time::OffsetDateTime::now_utc() - began < time::Duration::minutes(1));

    sqlx::query(
        "UPDATE listing_sources SET scrape_failing_since = $2 WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(time::OffsetDateTime::now_utc() - time::Duration::hours(3))
    .execute(&pool)
    .await
    .unwrap();
    let backdated = failing_since(&pool, listing_source_id).await.unwrap();

    let health = open(
        &service,
        listing_source_id,
        url,
        health,
        DomainFailureKind::Timeout,
    )
    .await;
    assert_eq!(health.scrape_failure_streak, 2);
    assert_eq!(
        failing_since(&pool, listing_source_id).await,
        Some(backdated),
        "a further failure of the same kind keeps the date"
    );

    let health = open(
        &service,
        listing_source_id,
        url,
        health,
        DomainFailureKind::Http429,
    )
    .await;
    assert_eq!(health.scrape_failure_streak, 1);
    let restarted = failing_since(&pool, listing_source_id).await.unwrap();
    assert!(
        restarted > backdated,
        "another kind restarts the streak and its date"
    );

    assert_eq!(
        service
            .close_scrape_circuit(&listing_source_id, &health)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );
    assert_eq!(failing_since(&pool, listing_source_id).await, None);
}

// ---------------------------------------------------------------------------
// get_random_product_urls_for_schema_seed — limit is respected
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_seed_urls_should_respect_limit() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());

    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "seed-limit.example.com").await;

    let current_url = "https://seed-limit.example.com/p/current";
    insert_product_url(&pool, listing_source_id, current_url).await;

    for i in 0..6u32 {
        insert_product_url(
            &pool,
            listing_source_id,
            &format!("https://seed-limit.example.com/p/{i}"),
        )
        .await;
    }

    let exclude = url::Url::parse(current_url).unwrap();
    let sampled = service
        .get_random_product_urls_for_schema_seed(&listing_source_id, &[exclude.to_string()], 3)
        .await
        .unwrap();

    assert_eq!(sampled.len(), 3, "query should respect the requested LIMIT");
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_scrape_failure_streaks_are_same_kind_consecutive_and_reset_after_recovery() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-streak-semantics.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-streak-semantics.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let first_next = time::OffsetDateTime::from_unix_timestamp(2_000_000_000).unwrap();
    let second_next = time::OffsetDateTime::from_unix_timestamp(2_000_000_100).unwrap();
    let third_next = time::OffsetDateTime::from_unix_timestamp(2_000_000_200).unwrap();
    let initial = ScrapeHealthSnapshot {
        scrape_failure_streak: 0,
        last_scrape_error_kind: None,
        next_scrape_at: None,
    };

    let first = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "rate limited",
                Some(429),
                first_next,
                None,
            ),
            initial.clone(),
            DomainFailureKind::Http429,
            Some(429),
            first_next,
        ))
        .await
        .unwrap();
    assert_eq!(first.url_failure, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(first.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(first.persisted_failure_streak, Some(1));

    let after_first = ScrapeHealthSnapshot {
        scrape_failure_streak: 1,
        last_scrape_error_kind: Some("HTTP_429".to_owned()),
        next_scrape_at: Some(first_next),
    };
    let second = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "rate limited again",
                Some(429),
                second_next,
                None,
            ),
            after_first.clone(),
            DomainFailureKind::Http429,
            Some(429),
            second_next,
        ))
        .await
        .unwrap();
    assert_eq!(second.persisted_failure_streak, Some(2));

    let after_second = ScrapeHealthSnapshot {
        scrape_failure_streak: 2,
        last_scrape_error_kind: Some("HTTP_429".to_owned()),
        next_scrape_at: Some(second_next),
    };
    let changed_kind = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "Timeout",
                "timed out",
                None,
                third_next,
                None,
            ),
            after_second.clone(),
            DomainFailureKind::Timeout,
            None,
            third_next,
        ))
        .await
        .unwrap();
    assert_eq!(changed_kind.persisted_failure_streak, Some(1));

    let recovering = ScrapeHealthSnapshot {
        scrape_failure_streak: 1,
        last_scrape_error_kind: Some("TIMEOUT".to_owned()),
        next_scrape_at: Some(third_next),
    };
    assert_eq!(
        service
            .close_scrape_circuit(&listing_source_id, &recovering)
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    let row: (i32, Option<String>, Option<time::OffsetDateTime>) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, next_scrape_at
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (0, None, None));

    let after_recovery = ScrapeHealthSnapshot {
        scrape_failure_streak: 0,
        last_scrape_error_kind: None,
        next_scrape_at: None,
    };
    let restarted = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "rate limited after recovery",
                Some(429),
                first_next,
                None,
            ),
            after_recovery,
            DomainFailureKind::Http429,
            Some(429),
            first_next,
        ))
        .await
        .unwrap();
    assert_eq!(restarted.persisted_failure_streak, Some(1));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_stale_scrape_failure_does_not_overwrite_kind_compatible_state() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-stale-circuit-fence.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-stale-circuit-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    let observed_next = time::OffsetDateTime::from_unix_timestamp(2_100_000_000).unwrap();
    sqlx::query(
        "UPDATE listing_sources
         SET scrape_failure_streak = 2,
             last_scrape_error_kind = 'HTTP_429',
             next_scrape_at = $2
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(observed_next)
    .execute(&pool)
    .await
    .unwrap();

    let expected = ScrapeHealthSnapshot {
        scrape_failure_streak: 2,
        last_scrape_error_kind: Some("HTTP_429".to_owned()),
        next_scrape_at: Some(observed_next),
    };
    sqlx::query(
        "UPDATE listing_sources
         SET last_scrape_error_kind = 'HTTP_503'
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "stale worker",
                Some(429),
                observed_next,
                None,
            ),
            expected.clone(),
            DomainFailureKind::Http429,
            Some(429),
            time::OffsetDateTime::from_unix_timestamp(2_100_000_100).unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(outcome.url_failure, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::NoopStale);
    assert_eq!(outcome.persisted_failure_streak, None);

    let source_row: (i32, String, time::OffsetDateTime) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, next_scrape_at
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_row.0, 2);
    assert_eq!(source_row.1, "HTTP_503");
    assert_eq!(source_row.2, observed_next);

    let url_row: (String, Option<i32>) = sqlx::query_as(
        "SELECT last_error_kind, last_status_code
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(url_row, ("HTTP_429".to_owned(), Some(429)));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_stale_url_failure_does_not_block_current_circuit_open() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(
        &pool,
        listing_source_id,
        "scraper-stale-url-fence.example.com",
    )
    .await;
    let url = url::Url::parse("https://scraper-stale-url-fence.example.com/p/1").unwrap();
    insert_product_url(&pool, listing_source_id, url.as_str()).await;

    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("0a".repeat(32))
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE listing_source_urls
         SET last_captured_raw_input_sha256 = decode($2, 'hex')
         WHERE listing_source_id = $1 AND url = $3",
    )
    .bind(listing_source_id.as_uuid())
    .bind("0b".repeat(32))
    .bind(url.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let next_scrape_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    let outcome = service
        .mark_fetch_failure_and_open_scrape_circuit(circuit_open_request(
            fetch_failure_request(
                listing_source_id,
                url.clone(),
                "HTTP_429",
                "stale URL worker",
                Some(429),
                next_scrape_at,
                Some(vec![0x0a; 32]),
            ),
            ScrapeHealthSnapshot {
                scrape_failure_streak: 0,
                last_scrape_error_kind: None,
                next_scrape_at: None,
            },
            DomainFailureKind::Http429,
            Some(429),
            next_scrape_at,
        ))
        .await
        .unwrap();

    assert_eq!(outcome.url_failure, CrawlerUrlWriteOutcome::NoopStale);
    assert_eq!(outcome.scrape_circuit, CrawlerUrlWriteOutcome::Applied);
    assert_eq!(outcome.persisted_failure_streak, Some(1));

    let url_row: (i32, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT failure_count, last_error_kind, last_status_code
         FROM listing_source_urls WHERE listing_source_id = $1 AND url = $2",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(url_row, (0, None, None));

    let source_row: (i32, Option<String>, Option<i32>) = sqlx::query_as(
        "SELECT scrape_failure_streak, last_scrape_error_kind, last_scrape_status_code
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_row, (1, Some("HTTP_429".to_owned()), Some(429)));
}

/// A URL retired as a duplicate of a stored page is terminal: neither scraped nor counted as due.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_never_select_a_url_retired_as_duplicate() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "duplicate-retired.example.com").await;
    let duplicate =
        url::Url::parse("https://duplicate-retired.example.com/collections/a/products/1").unwrap();
    let destination = url::Url::parse("https://duplicate-retired.example.com/products/1").unwrap();
    insert_product_url(&pool, listing_source_id, duplicate.as_str()).await;
    insert_product_url(&pool, listing_source_id, destination.as_str()).await;

    assert_eq!(
        service
            .set_disposition(
                &listing_source_id,
                &duplicate,
                CrawlerDisposition::Duplicate,
                None
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::Applied
    );

    assert_eq!(
        service
            .get_candidates(10, 100, &[])
            .await
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.url)
            .collect::<Vec<_>>(),
        vec![destination]
    );
    assert_eq!(service.count_due_urls().await.unwrap(), 1);
    assert_eq!(
        service
            .set_disposition(
                &listing_source_id,
                &duplicate,
                CrawlerDisposition::Active,
                None
            )
            .await
            .unwrap(),
        CrawlerUrlWriteOutcome::NoopStale,
        "no scraper write revives a retired duplicate"
    );
}

// ---------------------------------------------------------------------------
// get_candidates — a schema review wait holds its ListingSource back
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_not_select_a_source_while_a_page_waits_on_a_schema_review_cooldown() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "review-hold.example.com").await;
    let waiting_url = "https://review-hold.example.com/p/waiting";
    insert_product_url(&pool, listing_source_id, waiting_url).await;
    insert_product_url(
        &pool,
        listing_source_id,
        "https://review-hold.example.com/p/other",
    )
    .await;
    let review_id = insert_schemas_and_pending_review(&pool, listing_source_id).await;
    service
        .mark_waiting_on_schema_review(SchemaReviewWaitRequest {
            url_failure: fetch_failure_request(
                listing_source_id,
                url::Url::parse(waiting_url).unwrap(),
                "PendingSchemaReview",
                "blocked pending product schema review",
                None,
                time::OffsetDateTime::now_utc() + time::Duration::minutes(30),
                None,
            ),
            wait: SchemaReviewWait {
                review_id,
                streak: 1,
            },
        })
        .await
        .unwrap();

    assert!(service.get_candidates(5, 5, &[]).await.unwrap().is_empty());

    sqlx::query(
        "UPDATE listing_source_urls SET next_retry_at = NOW() - INTERVAL '1 minute' WHERE url = $1",
    )
    .bind(waiting_url)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(service.get_candidates(5, 5, &[]).await.unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// url_carrying_source_listing_id
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn scraper_should_find_the_earliest_live_url_of_the_source_carrying_an_identity() {
    let pool = get_postgres_client().await;
    let service = ScraperCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id, "identity.example.com").await;
    let other_source_id = ListingSourceId::new();
    insert_listing_source(&pool, other_source_id, "other-identity.example.com").await;
    let retired = url::Url::parse("https://identity.example.com/view?p_idx=7&sorting=a").unwrap();
    let first = url::Url::parse("https://identity.example.com/view?p_idx=7").unwrap();
    let second = url::Url::parse("https://identity.example.com/view?p_idx=7&page=2").unwrap();
    let foreign = url::Url::parse("https://other-identity.example.com/view?p_idx=7").unwrap();
    for (source, url, disposition) in [
        (listing_source_id, &retired, CrawlerDisposition::Duplicate),
        (listing_source_id, &first, CrawlerDisposition::Active),
        (listing_source_id, &second, CrawlerDisposition::Active),
        (other_source_id, &foreign, CrawlerDisposition::Active),
    ] {
        insert_product_url(&pool, source, url.as_str()).await;
        sqlx::query(
            "UPDATE listing_source_urls SET last_source_listing_id = '7', crawler_disposition = $2 WHERE url = $1",
        )
        .bind(url.as_str())
        .bind(disposition.as_str())
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE listing_source_urls SET created = NOW() - INTERVAL '1 day' WHERE url = $1")
        .bind(first.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let carrier = |identity: &'static str, excluding: &url::Url| {
        let service = &service;
        let excluding = excluding.clone();
        async move {
            service
                .url_carrying_source_listing_id(&listing_source_id, identity, &excluding)
                .await
                .unwrap()
        }
    };

    assert_eq!(carrier("7", &second).await, Some(first.clone()));
    assert_eq!(carrier("7", &first).await, Some(second.clone()));
    assert_eq!(carrier("8", &second).await, None);
}
