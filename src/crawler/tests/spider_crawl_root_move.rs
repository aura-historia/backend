//! A scope sync that gives a ListingSource another crawl root while a crawl of the old root runs:
//! every write of that crawl's outcome records nothing over the move, and the crawls of the new
//! root record theirs (`docs/crawler/README.md#listingsource-scope`).

use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    ListingSourceSnapshotResult, ScopedListingSource,
};
use crawler::spider::candidate_service::{
    CrawlRateLimitWrite, SpiderCandidateService, SpiderCandidateServiceImpl,
    UrlPatternReviewWaitWrite,
};
use crawler::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, ListingSourceUrlPatternRepositoryImpl,
};
use crawler::spider::crawl_frontier_repository::{
    CrawlFrontierRepository, CrawlFrontierRepositoryImpl,
};
use crawler::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use crawler::spider::discovery::frontier::CrawlKind;
use crawler::spider::discovery::website_spider::CrawlCompletion;
use crawler::{CrawlRoot, CrawlerReviewId};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const OLD_ROOT: &str = "https://moving-shop.example.com/";
const NEW_ROOT: &str = "https://www.moving-shop.example.com/";

/// Applies a scope that holds only the crawl-enabled source `listing_source_id`, on `root`.
async fn apply_scope(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    root: &str,
) -> ListingSourceSnapshotResult {
    ListingSourceRegistrationRepositoryImpl::new(pool.clone())
        .apply_snapshot(&[ScopedListingSource {
            listing_source_id,
            listing_source_name: ListingSourceName::try_from("Moving shop").unwrap(),
            listing_source_slug: ListingSourceSlugId::raw("moving-shop").unwrap(),
            crawl_enabled: true,
            crawl_root: Some(CrawlRoot::from_presentation_url(root).unwrap()),
        }])
        .await
        .unwrap()
}

/// A crawl-enabled source on `root` that was never crawled.
async fn insert_listing_source(pool: &PgPool, root: &str) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    let applied = apply_scope(pool, listing_source_id, root).await;
    assert_eq!(
        applied.moved_crawl_roots, 0,
        "a new source has no root to move"
    );
    listing_source_id
}

/// The crawl root a spider run of the source's next crawl would crawl.
async fn next_crawl_root(pool: &PgPool) -> (CrawledRoot, CrawlKind) {
    let candidates = SpiderCandidateServiceImpl::new(pool.clone())
        .get_candidates(10, &[])
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1, "the source is due");
    (candidates[0].crawled_root(), candidates[0].kind)
}

async fn insert_pending_url_pattern_review(
    pool: &PgPool,
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

/// Every column a crawl's outcome writes.
#[derive(Debug, PartialEq, sqlx::FromRow)]
struct CrawlState {
    crawl_root_url: String,
    last_crawled: Option<OffsetDateTime>,
    crawl_failure_count: i32,
    last_crawl_error_kind: Option<String>,
    last_crawl_outcome: Option<String>,
    crawl_truncation_count: i32,
    crawl_rate_limit_streak: i32,
    crawl_rate_limited_until: Option<OffsetDateTime>,
    next_crawl_at: Option<OffsetDateTime>,
    last_crawl_run_at: Option<OffsetDateTime>,
}

impl CrawlState {
    /// The state of a source on `crawl_root_url` that was never crawled.
    fn never_crawled(crawl_root_url: &str) -> Self {
        Self {
            crawl_root_url: crawl_root_url.to_owned(),
            last_crawled: None,
            crawl_failure_count: 0,
            last_crawl_error_kind: None,
            last_crawl_outcome: None,
            crawl_truncation_count: 0,
            crawl_rate_limit_streak: 0,
            crawl_rate_limited_until: None,
            next_crawl_at: None,
            last_crawl_run_at: None,
        }
    }

    async fn of(pool: &PgPool, listing_source_id: ListingSourceId) -> Self {
        sqlx::query_as(
            "SELECT crawl_root_url, last_crawled, crawl_failure_count, last_crawl_error_kind, \
                    last_crawl_outcome, crawl_truncation_count, crawl_rate_limit_streak, \
                    crawl_rate_limited_until, next_crawl_at, last_crawl_run_at \
             FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
    }
}

async fn frontier(pool: &PgPool, listing_source_id: ListingSourceId) -> Vec<Url> {
    CrawlFrontierRepositoryImpl::new(pool.clone())
        .load_frontier(&listing_source_id)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_record_only_the_outcomes_of_crawls_of_the_source_s_current_crawl_root() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool, OLD_ROOT).await;
    let (old_root, _) = next_crawl_root(&pool).await;
    assert_eq!(old_root, CrawledRoot::new(listing_source_id, OLD_ROOT));
    let review_id = insert_pending_url_pattern_review(&pool, listing_source_id).await;
    assert_eq!(
        CrawlFrontierRepositoryImpl::new(pool.clone())
            .replace_frontier(
                &old_root,
                &[Url::parse(&format!("{OLD_ROOT}category/1")).unwrap()],
            )
            .await
            .unwrap(),
        CrawlOutcomeWrite::Recorded,
        "the old root's crawl leaves a frontier before the move"
    );

    let moved = apply_scope(&pool, listing_source_id, NEW_ROOT).await;
    assert_eq!(moved.moved_crawl_roots, 1);
    assert_eq!(
        CrawlState::of(&pool, listing_source_id).await,
        CrawlState::never_crawled(NEW_ROOT)
    );
    assert!(
        frontier(&pool, listing_source_id).await.is_empty(),
        "the move drops the old root's frontier"
    );

    let candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let in_a_day = OffsetDateTime::now_utc() + time::Duration::days(1);
    let rate_limit = CrawlRateLimitWrite {
        streak: 1,
        not_before: in_a_day,
    };
    let old_root_writes = [
        (
            "completed",
            candidates
                .mark_crawl_completed(&old_root, CrawlCompletion::Completed)
                .await,
        ),
        (
            "truncated",
            candidates
                .mark_crawl_truncated(&old_root, 1, in_a_day)
                .await,
        ),
        (
            "incremental",
            candidates
                .mark_incremental_crawl_finished(&old_root, CrawlCompletion::Completed)
                .await,
        ),
        (
            "rate-limited",
            candidates
                .mark_crawl_rate_limited(&old_root, rate_limit)
                .await,
        ),
        (
            "failed",
            candidates
                .mark_crawl_failure(&old_root, "NoUrlPattern", 4, Some(rate_limit), in_a_day)
                .await,
        ),
        (
            "stopped for an operator",
            candidates
                .mark_crawl_needs_operator(&old_root, "NoUrlPattern", 5)
                .await,
        ),
        (
            "frontier",
            CrawlFrontierRepositoryImpl::new(pool.clone())
                .replace_frontier(
                    &old_root,
                    &[Url::parse(&format!("{OLD_ROOT}category/2")).unwrap()],
                )
                .await,
        ),
        (
            "full crawl",
            ListingSourceUrlPatternRepositoryImpl::new(pool.clone())
                .mark_as_crawled(&old_root)
                .await,
        ),
    ];
    for (write, result) in old_root_writes {
        assert_eq!(
            result.unwrap(),
            CrawlOutcomeWrite::RootMoved,
            "the {write} write of the old root's crawl"
        );
    }
    assert_eq!(
        candidates
            .mark_crawl_awaiting_url_pattern_review(
                &old_root,
                &review_id,
                Some(rate_limit),
                in_a_day
            )
            .await
            .unwrap(),
        UrlPatternReviewWaitWrite::RootMoved,
        "the review is pending, but the source has another root"
    );
    assert_eq!(
        CrawlState::of(&pool, listing_source_id).await,
        CrawlState::never_crawled(NEW_ROOT),
        "the move's crawl state survives every write of the old root's crawl"
    );
    assert!(frontier(&pool, listing_source_id).await.is_empty());

    let (new_root, kind) = next_crawl_root(&pool).await;
    assert_eq!(
        (new_root.clone(), kind),
        (
            CrawledRoot::new(listing_source_id, NEW_ROOT),
            CrawlKind::Full
        )
    );
    assert_eq!(
        CrawlFrontierRepositoryImpl::new(pool.clone())
            .replace_frontier(
                &new_root,
                &[Url::parse(&format!("{NEW_ROOT}category/2")).unwrap()],
            )
            .await
            .unwrap(),
        CrawlOutcomeWrite::Recorded
    );
    assert_eq!(
        ListingSourceUrlPatternRepositoryImpl::new(pool.clone())
            .mark_as_crawled(&new_root)
            .await
            .unwrap(),
        CrawlOutcomeWrite::Recorded
    );
    assert_eq!(
        candidates
            .mark_crawl_completed(&new_root, CrawlCompletion::Completed)
            .await
            .unwrap(),
        CrawlOutcomeWrite::Recorded
    );
    let recorded = CrawlState::of(&pool, listing_source_id).await;
    assert_eq!(recorded.crawl_root_url, NEW_ROOT);
    assert_eq!(recorded.last_crawl_outcome.as_deref(), Some("COMPLETED"));
    assert!(
        recorded.last_crawled.is_some(),
        "a full crawl of the new root is recorded"
    );
    assert_eq!(
        frontier(&pool, listing_source_id).await,
        [Url::parse(&format!("{NEW_ROOT}category/2")).unwrap()]
    );
}

/// Waits until `count` statements are blocked on a lock.
async fn wait_for_lock_waiters(pool: &PgPool, count: i64) {
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{count} statements never waited on a lock");
}

/// The old root's crawl replaces its frontier while the move to the new root commits: the
/// replacement waits for the move, then finds the source on the new root, so no frontier of the
/// old root outlives the move that dropped the old one.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_no_frontier_of_the_old_root_when_the_move_commits_while_it_is_written() {
    let pool = get_postgres_client().await;
    let listing_source_id = insert_listing_source(&pool, OLD_ROOT).await;
    let (old_root, _) = next_crawl_root(&pool).await;
    // Holding the source's row queues the move, then the frontier write, behind it.
    let mut source_lock = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE")
        .bind(listing_source_id.as_uuid())
        .execute(&mut *source_lock)
        .await
        .unwrap();
    let moving = tokio::spawn({
        let pool = pool.clone();
        async move { apply_scope(&pool, listing_source_id, NEW_ROOT).await }
    });
    wait_for_lock_waiters(&pool, 1).await;
    let replacing = tokio::spawn({
        let pool = pool.clone();
        let old_root = old_root.clone();
        async move {
            CrawlFrontierRepositoryImpl::new(pool)
                .replace_frontier(
                    &old_root,
                    &[Url::parse(&format!("{OLD_ROOT}category/2")).unwrap()],
                )
                .await
                .unwrap()
        }
    });
    wait_for_lock_waiters(&pool, 2).await;

    source_lock.commit().await.unwrap();

    assert_eq!(moving.await.unwrap().moved_crawl_roots, 1);
    assert_eq!(
        CrawlState::of(&pool, listing_source_id)
            .await
            .crawl_root_url,
        NEW_ROOT
    );
    assert_eq!(replacing.await.unwrap(), CrawlOutcomeWrite::RootMoved);
    assert!(
        frontier(&pool, listing_source_id).await.is_empty(),
        "the old root's frontier must not be written after the move dropped the frontier"
    );
}
