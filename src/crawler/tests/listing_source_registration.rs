//! The scope sync against PostgreSQL: the crawl root each ListingSource's presentation URL
//! derives, its moves, and the one-snapshot-at-a-time transaction
//! (`docs/crawler/README.md#listingsource-scope`).

use async_trait::async_trait;
use crawler::CrawlRoot;
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    ListingSourceRegistrationService, ListingSourceRegistrationSource, ListingSourceSyncError,
    RegisteredListingSource, ScopedListingSource,
};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::spider::crawl_frontier_repository::{
    CrawlFrontierRepository, CrawlFrontierRepositoryImpl,
};
use crawler::spider::crawled_root::CrawledRoot;
use crawler::spider::discovery::frontier::CrawlKind;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

type ListingSourceRow = (
    uuid::Uuid,
    String,
    String,
    bool,
    time::OffsetDateTime,
    time::OffsetDateTime,
);

/// A source's crawl root, URL pattern and crawl state, as the snapshot leaves them.
type CrawlStateRow = (
    uuid::Uuid,
    Option<String>,
    Option<String>,
    String,
    Option<time::OffsetDateTime>,
    i32,
    Option<String>,
    Option<time::OffsetDateTime>,
);

fn crawl_root(presentation_url: &str) -> CrawlRoot {
    CrawlRoot::from_presentation_url(presentation_url)
        .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"))
}

fn scoped(
    listing_source_id: ListingSourceId,
    crawl_enabled: bool,
    presentation_url: Option<&str>,
) -> ScopedListingSource {
    ScopedListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled,
        crawl_root: presentation_url.map(crawl_root),
    }
}

fn registered(
    listing_source_id: ListingSourceId,
    presentation_url: Option<&str>,
) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled: true,
        presentation_url: presentation_url.map(str::to_owned),
    }
}

/// The admin API scope a sync reads, as fixed records.
struct FixedScope(Vec<RegisteredListingSource>);

#[async_trait]
impl ListingSourceRegistrationSource for FixedScope {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(self.0.clone())
    }
}

async fn sync(pool: &PgPool, scope: Vec<RegisteredListingSource>) {
    ListingSourceRegistrationService::new(
        Box::new(FixedScope(scope)),
        Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone())),
    )
    .sync()
    .await
    .unwrap_or_else(|error| panic!("sync the scope: {error}"));
}

/// The roots the spider would crawl now, by ListingSource.
async fn due_crawl_roots(pool: &PgPool) -> Vec<(ListingSourceId, String)> {
    let mut roots: Vec<_> = SpiderCandidateServiceImpl::new(pool.clone())
        .get_candidates(100, &[])
        .await
        .unwrap()
        .into_iter()
        .map(|candidate| {
            (
                candidate.listing_source_id,
                candidate.crawl_root.url().to_string(),
            )
        })
        .collect();
    roots.sort();
    roots
}

async fn crawl_state(pool: &PgPool, listing_source_id: ListingSourceId) -> CrawlStateRow {
    sqlx::query_as(
        "SELECT listing_source_id, crawl_root_url, url_pattern, url_pattern_state, last_crawled, \
                crawl_failure_count, last_crawl_error_kind, next_crawl_at \
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Gives the source a URL pattern, a stored URL and crawl state as a crawl of `root` leaves them,
/// and a frontier of `root`.
async fn crawl_once(pool: &PgPool, listing_source_id: ListingSourceId, root: &str) {
    sqlx::query(
        "UPDATE listing_sources \
         SET url_pattern = '/item/', url_pattern_state = 'MATCHED', url_pattern_matching = 'FULL_URL_SEARCH', \
             last_crawled = NOW() - INTERVAL '1 day', crawl_failure_count = 2, \
             last_crawl_error_kind = 'HTTP_500', next_crawl_at = NOW() + INTERVAL '1 hour', \
             last_crawl_outcome = 'FAILED', last_crawl_run_at = NOW() - INTERVAL '1 hour' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_urls \
             (listing_source_id, url, raw_source_record_key, url_class) \
         VALUES ($1, $2, $2, 'product')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("{root}item/1"))
    .execute(pool)
    .await
    .unwrap();
    let frontier = [Url::parse(&format!("{root}category/2")).unwrap()];
    CrawlFrontierRepositoryImpl::new(pool.clone())
        .replace_frontier(&CrawledRoot::new(listing_source_id, root), &frontier)
        .await
        .unwrap();
}

async fn stored_urls(pool: &PgPool, listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A crawler database bootstrapped against the admin API crawls every enabled source whose
/// presentation URL names a host of its own, from that URL, without an operator registering
/// anything; a source on a host another source presents, or on no domain host, is not crawled.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_crawl_each_source_of_a_fresh_scope_from_its_own_presentation_url() {
    let pool = get_postgres_client().await;
    let shop = ListingSourceId::new();
    let shop_with_path = ListingSourceId::new();
    let marketplace_seller_a = ListingSourceId::new();
    let marketplace_seller_b = ListingSourceId::new();
    let ip_host = ListingSourceId::new();
    let no_url = ListingSourceId::new();

    sync(
        &pool,
        vec![
            registered(shop, Some("http://www.shop-a.example")),
            registered(
                shop_with_path,
                Some("https://shop-b.example/en/antiques/?ref=api"),
            ),
            registered(
                marketplace_seller_a,
                Some("https://www.marketplace.example/seller/a"),
            ),
            registered(
                marketplace_seller_b,
                Some("https://marketplace.example/seller/b"),
            ),
            registered(ip_host, Some("https://203.0.113.7/")),
            registered(no_url, None),
        ],
    )
    .await;

    let mut expected = vec![
        (shop, "https://www.shop-a.example/".to_owned()),
        (
            shop_with_path,
            "https://shop-b.example/en/antiques/".to_owned(),
        ),
    ];
    expected.sort();
    assert_eq!(due_crawl_roots(&pool).await, expected);
    for skipped in [marketplace_seller_a, marketplace_seller_b, ip_host, no_url] {
        let (_, root, ..) = crawl_state(&pool, skipped).await;
        assert_eq!(root, None, "a skipped source has no crawl root");
    }
}

/// A changed presentation URL moves the source's crawl root on the next refresh: its crawl state
/// and frontier are reset, while its URLs, URL pattern and scrape state stay.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_move_the_crawl_root_and_reset_crawl_state_when_the_presentation_url_changes() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    let old_root = "https://shop.example/";
    sync(&pool, vec![registered(listing_source_id, Some(old_root))]).await;
    crawl_once(&pool, listing_source_id, old_root).await;
    let before_unchanged_refresh = crawl_state(&pool, listing_source_id).await;

    sync(&pool, vec![registered(listing_source_id, Some(old_root))]).await;
    assert_eq!(
        crawl_state(&pool, listing_source_id).await,
        before_unchanged_refresh,
        "a refresh with the same URL keeps the crawl state"
    );

    sync(
        &pool,
        vec![registered(
            listing_source_id,
            Some("https://www.shop.example/en/"),
        )],
    )
    .await;

    assert_eq!(
        crawl_state(&pool, listing_source_id).await,
        (
            *listing_source_id.as_uuid(),
            Some("https://www.shop.example/en/".to_owned()),
            Some("/item/".to_owned()),
            "MATCHED".to_owned(),
            None,
            0,
            None,
            None,
        )
    );
    assert!(
        CrawlFrontierRepositoryImpl::new(pool.clone())
            .load_frontier(&listing_source_id)
            .await
            .unwrap()
            .is_empty(),
        "the old root's frontier is dropped"
    );
    assert_eq!(stored_urls(&pool, listing_source_id).await, 1);
    let candidates = SpiderCandidateServiceImpl::new(pool.clone())
        .get_candidates(10, &[])
        .await
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        (candidates[0].crawl_root.url().as_str(), candidates[0].kind),
        ("https://www.shop.example/en/", CrawlKind::Full),
        "the next spider cycle crawls the new root in full"
    );
}

/// A source that leaves the scope keeps its crawl root, crawl state and URLs with crawling
/// disabled, and comes back with them when the scope holds it again with the same URL.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_history_of_a_source_that_leaves_and_returns_to_the_scope() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    let root = "https://shop.example/";
    sync(&pool, vec![registered(listing_source_id, Some(root))]).await;
    crawl_once(&pool, listing_source_id, root).await;
    let crawled = crawl_state(&pool, listing_source_id).await;

    sync(&pool, vec![]).await;

    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
    assert_eq!(crawl_state(&pool, listing_source_id).await, crawled);
    assert_eq!(stored_urls(&pool, listing_source_id).await, 1);
    assert!(due_crawl_roots(&pool).await.is_empty());

    sync(&pool, vec![registered(listing_source_id, Some(root))]).await;
    assert_eq!(crawl_state(&pool, listing_source_id).await, crawled);
}

/// One snapshot that swaps two crawl hosts between sources, or hands a disabled source's host to
/// another source, applies without tripping the one-host-per-enabled-source index.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_apply_a_snapshot_that_swaps_crawl_hosts_between_sources() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let first = ListingSourceId::new();
    let second = ListingSourceId::new();
    let leaving = ListingSourceId::new();
    repository
        .apply_snapshot(&[
            scoped(first, true, Some("https://a.example/")),
            scoped(second, true, Some("https://b.example/")),
            scoped(leaving, true, Some("https://c.example/")),
        ])
        .await
        .unwrap();

    let swapped = repository
        .apply_snapshot(&[
            scoped(first, true, Some("https://b.example/")),
            scoped(second, true, Some("https://c.example/")),
        ])
        .await
        .unwrap();

    assert_eq!((swapped.disabled, swapped.moved_crawl_roots), (1, 2));
    let mut expected = vec![
        (first, "https://b.example/".to_owned()),
        (second, "https://c.example/".to_owned()),
    ];
    expected.sort();
    assert_eq!(due_crawl_roots(&pool).await, expected);
}

/// The schema keeps two crawl-enabled sources off one crawl host, whatever writes them.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_second_crawl_enabled_source_on_one_crawl_host() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());

    let refused = repository
        .apply_snapshot(&[
            scoped(ListingSourceId::new(), true, Some("https://shop.example/")),
            scoped(
                ListingSourceId::new(),
                true,
                Some("https://www.shop.example/"),
            ),
        ])
        .await;

    assert!(refused.is_err());
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listing_sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sources, 0, "the refused snapshot commits nothing");
}

/// A stored crawl root that is not the one its URL derives is invalid crawler state: candidate
/// selection fails closed instead of crawling it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_fail_closed_on_a_persisted_crawl_root_its_url_does_not_derive() {
    let pool = get_postgres_client().await;
    sqlx::query(
        "INSERT INTO listing_sources \
             (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
              crawl_root_url, crawl_host) \
         VALUES ($1, 'Corrupt', 'corrupt', TRUE, 'https://shop.example/', 'other.example')",
    )
    .bind(ListingSourceId::new().as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let candidates = SpiderCandidateServiceImpl::new(pool.clone())
        .get_candidates(10, &[])
        .await;

    assert!(matches!(candidates, Err(sqlx::Error::Decode(_))));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_default_ad_hoc_listing_sources_to_crawl_disabled() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug) \
         VALUES ($1, 'Ad hoc', 'ad-hoc')",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let enabled: bool = sqlx::query_scalar(
        "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!enabled);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_roll_back_snapshot_when_final_disable_update_fails_and_retry_after_trigger_removal()
{
    let pool = get_postgres_client().await;
    let repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let kept = ListingSourceId::new();
    let removed = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, url_pattern, url_pattern_state, url_pattern_matching, \
          last_crawled, \
          crawl_failure_count, last_crawl_error_kind, next_crawl_at, created, updated) \
         VALUES \
         ($1, 'Old kept source', 'old-kept-source', TRUE, \
          'https://kept.example.com/', 'kept.example.com', '/products/', 'MATCHED', 'FULL_URL_SEARCH', \
          NOW() - INTERVAL '1 day', 3, 'HTTP_500', NOW() + INTERVAL '1 hour', NOW(), NOW()), \
         ($2, 'Removed source', 'removed-source', TRUE, \
          'https://removed.example.com/', 'removed.example.com', '/stock/', 'MATCHED', 'FULL_URL_SEARCH', \
          NOW() - INTERVAL '2 days', 4, 'TIMEOUT', NOW() + INTERVAL '2 hours', NOW(), NOW())",
    )
    .bind(kept.as_uuid())
    .bind(removed.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let source_rows = || async {
        sqlx::query_as::<_, ListingSourceRow>(
            "SELECT listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
                    created, updated \
             FROM listing_sources ORDER BY listing_source_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let crawl_states = || async {
        let mut states = vec![
            crawl_state(&pool, kept).await,
            crawl_state(&pool, removed).await,
        ];
        states.sort();
        states
    };
    let source_rows_before = source_rows().await;
    let crawl_states_before = crawl_states().await;
    let snapshot = [scoped(kept, true, Some("https://kept.example.com/"))];

    sqlx::query(
        "CREATE FUNCTION fail_listing_source_snapshot_disable() RETURNS trigger \
         LANGUAGE plpgsql AS $$ \
         BEGIN \
             RAISE EXCEPTION 'injected listing source disable failure'; \
         END; \
         $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_listing_source_snapshot_disable \
         BEFORE UPDATE OF crawl_enabled ON listing_sources \
         FOR EACH ROW \
         WHEN (OLD.crawl_enabled IS TRUE AND NEW.crawl_enabled IS FALSE) \
         EXECUTE FUNCTION fail_listing_source_snapshot_disable()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let failed_snapshot = repository.apply_snapshot(&snapshot).await;

    sqlx::query("DROP TRIGGER fail_listing_source_snapshot_disable ON listing_sources")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_listing_source_snapshot_disable()")
        .execute(&pool)
        .await
        .unwrap();

    assert!(failed_snapshot.is_err());
    assert_eq!(source_rows().await, source_rows_before);
    assert_eq!(crawl_states().await, crawl_states_before);

    let retry = repository.apply_snapshot(&snapshot).await;
    assert!(matches!(retry, Ok(result) if result.disabled == 1 && result.moved_crawl_roots == 0));
    let enabled: Vec<(uuid::Uuid, String, bool)> = sqlx::query_as(
        "SELECT listing_source_id, listing_source_name, crawl_enabled \
         FROM listing_sources ORDER BY listing_source_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(enabled.contains(&(*kept.as_uuid(), "Test source".to_owned(), true)));
    assert!(enabled.contains(&(*removed.as_uuid(), "Removed source".to_owned(), false)));
    assert_eq!(
        crawl_states().await,
        crawl_states_before,
        "neither the kept nor the disabled source's crawl state changes"
    );
}
