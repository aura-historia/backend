//! The review console's lists and counts as the readers answer them from SQL: the paginated review
//! queue and ListingSource overview with their filters, search and sort, one ListingSource's page,
//! and the counts of the first page. The query parsing and the HTTP answers are proven at their own
//! layers; these tests prove the SQL against a database.

use crawler::CrawlerReviewId;
use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::model::{
    ArtifactType, ListingSourcePage, PAGE_ROLE_PRIMARY, PageSize, ReviewActor, ReviewQueuePage,
    ReviewStatus, SchemaReviewPageInput, SearchTerm, SortDirection, SourceActivity, SourceHealth,
    SourceSort, UrlPatternDecision,
};
use crawler::review::readers::{
    ConsoleOverviewReader, ListingSourceDetailReader, ListingSourceOverviewReader,
    ListingSourceQuery, PageReadError, ReviewHistoryQuery, ReviewQuery, ReviewQueueReader,
};
use crawler::review::repository::{CrawlerReviewRepository, SchemaReviewWithStatusInput};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::spider::crawled_root::CrawledRoot;
use crawler::spider::local_lock::{LocalLockManager, WorkLock, WorkLockScope};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use std::num::NonZeroU32;
use std::sync::Arc;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

fn schema() -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: rule("h1"),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: rule("span.state"),
        images: ExtractionRule {
            selector: "img.product".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        },
        raw_attributes: Default::default(),
    }
}

fn review_pages() -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: "https://example.com/products/1".to_string(),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: "<html><body><h1>Title</h1></body></html>".to_string(),
    }]
}

fn page_size(size: usize) -> PageSize {
    PageSize::new(size).unwrap_or_else(|| panic!("{size} is a page size"))
}

/// A ListingSource named `name` with the slug `slug` and the crawl host `host`.
async fn insert_source(
    pool: &PgPool,
    name: &str,
    slug: &str,
    host: &str,
    crawl_enabled: bool,
) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(name)
    .bind(slug)
    .bind(crawl_enabled)
    .bind(format!("https://{host}/"))
    .bind(host)
    .execute(pool)
    .await
    .unwrap();
    listing_source_id
}

async fn pending_schema_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
) -> CrawlerReviewId {
    repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap()
}

async fn closed_schema_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
    status: &str,
) -> CrawlerReviewId {
    repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "initial_schema_generation",
            schemas: &[schema()],
            pages: review_pages(),
            validation_summary: json!({}),
            status,
            notes: None,
        })
        .await
        .unwrap()
}

async fn pending_url_pattern_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
) -> CrawlerReviewId {
    repository
        .create_url_pattern_review(
            &listing_source_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &["https://example.com/item/1".to_owned()],
            None,
        )
        .await
        .unwrap()
}

fn agent_repository(pool: &PgPool, max_repair_rounds: u32) -> CrawlerReviewRepository {
    CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(AgentDecisionPolicy::new(
        CrawlerAuthoringMode::Agent,
        AgentLimits {
            max_repair_rounds: NonZeroU32::new(max_repair_rounds).unwrap(),
            ..AgentLimits::default()
        },
    ))
}

async fn list_reviews(repository: &CrawlerReviewRepository, query: ReviewQuery) -> ReviewQueuePage {
    ReviewQueueReader::of(repository)
        .list_reviews(&query)
        .await
        .unwrap()
}

fn review_ids(page: &ReviewQueuePage) -> Vec<CrawlerReviewId> {
    page.reviews.iter().map(|review| review.review_id).collect()
}

/// Reads every page of the queue of `query` with `size` reviews a page, newest or oldest first as
/// `query` says, and returns each page's total and ids.
async fn read_all_reviews(
    repository: &CrawlerReviewRepository,
    query: ReviewQuery,
    size: usize,
) -> Vec<(i64, Vec<CrawlerReviewId>)> {
    let mut pages = Vec::new();
    let mut after = None;
    loop {
        let page = list_reviews(
            repository,
            ReviewQuery {
                after,
                page_size: page_size(size),
                ..query.clone()
            },
        )
        .await;
        pages.push((page.total, review_ids(&page)));
        match page.next_after {
            Some(next) => after = Some(next),
            None => return pages,
        }
    }
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_page_through_every_review_newest_first_without_gaps_or_repeats() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let mut created = Vec::new();
    for index in 0..4 {
        let source = insert_source(
            &pool,
            &format!("Shop {index}"),
            &format!("shop-{index}"),
            &format!("shop{index}.example.com"),
            true,
        )
        .await;
        created.push(closed_schema_review(&repository, source, "APPROVED").await);
        created.push(pending_url_pattern_review(&repository, source).await);
    }
    created.reverse();

    let pages = read_all_reviews(&repository, ReviewQuery::default(), 3).await;

    assert_eq!(
        pages
            .iter()
            .map(|(total, ids)| (*total, ids.len()))
            .collect::<Vec<_>>(),
        vec![(8, 3), (8, 3), (8, 2)]
    );
    assert_eq!(
        pages
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .collect::<Vec<_>>(),
        created,
        "newest first, each review once"
    );
    let mut oldest_first = created.clone();
    oldest_first.reverse();
    let ascending = read_all_reviews(
        &repository,
        ReviewQuery {
            direction: SortDirection::Ascending,
            ..ReviewQuery::default()
        },
        5,
    )
    .await;
    assert_eq!(
        ascending
            .into_iter()
            .flat_map(|(_, ids)| ids)
            .collect::<Vec<_>>(),
        oldest_first
    );
}

/// A review that opens, closes or is decided between two pages moves no other review across the
/// page boundary: the pages continue after the review the previous one ended with.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_pages_of_the_queue_stable_while_reviews_open_and_close() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let mut created = Vec::new();
    for index in 0..5 {
        let source = insert_source(
            &pool,
            &format!("Shop {index}"),
            &format!("shop-{index}"),
            &format!("shop{index}.example.com"),
            true,
        )
        .await;
        created.push(pending_schema_review(&repository, source).await);
    }
    created.reverse();
    let first = list_reviews(
        &repository,
        ReviewQuery {
            page_size: page_size(2),
            ..ReviewQuery::default()
        },
    )
    .await;
    assert_eq!(review_ids(&first), created[..2]);
    let late_source = insert_source(&pool, "Late", "late", "late.example.com", true).await;
    let late = pending_schema_review(&repository, late_source).await;
    repository
        .reject_review(created[3], ReviewActor::Human, Some("not a product schema"))
        .await
        .unwrap();

    let second = list_reviews(
        &repository,
        ReviewQuery {
            after: first.next_after,
            page_size: page_size(2),
            ..ReviewQuery::default()
        },
    )
    .await;

    assert_eq!(
        review_ids(&second),
        created[2..4],
        "the review decided in between stays where it was, the one opened since is not repeated"
    );
    assert_eq!(second.total, 6);
    assert!(!review_ids(&second).contains(&late));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_reviews_by_status_artifact_type_and_listing_source() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let first = insert_source(&pool, "First", "first", "first.example.com", true).await;
    let second = insert_source(&pool, "Second", "second", "second.example.com", true).await;
    let approved = closed_schema_review(&repository, first, "APPROVED").await;
    let rejected = closed_schema_review(&repository, first, "REJECTED").await;
    let pending_schema = pending_schema_review(&repository, first).await;
    let pending_pattern = pending_url_pattern_review(&repository, first).await;
    let other_pending = pending_schema_review(&repository, second).await;
    let ids = |page: &ReviewQueuePage| {
        let mut ids = review_ids(page);
        ids.sort_by_key(|id| id.to_string());
        ids
    };
    let sorted = |mut expected: Vec<CrawlerReviewId>| {
        expected.sort_by_key(|id| id.to_string());
        expected
    };

    let closed = list_reviews(
        &repository,
        ReviewQuery {
            statuses: vec![ReviewStatus::Approved, ReviewStatus::Rejected],
            ..ReviewQuery::default()
        },
    )
    .await;
    let patterns = list_reviews(
        &repository,
        ReviewQuery {
            artifact_type: Some(ArtifactType::UrlPattern),
            ..ReviewQuery::default()
        },
    )
    .await;
    let pending_of_first = list_reviews(
        &repository,
        ReviewQuery {
            listing_source_id: Some(first),
            statuses: vec![ReviewStatus::PendingReview],
            artifact_type: Some(ArtifactType::ProductSchema),
            ..ReviewQuery::default()
        },
    )
    .await;
    let all_of_second = list_reviews(
        &repository,
        ReviewQuery {
            listing_source_id: Some(second),
            ..ReviewQuery::default()
        },
    )
    .await;

    assert_eq!(
        (closed.total, ids(&closed)),
        (2, sorted(vec![approved, rejected]))
    );
    assert_eq!((patterns.total, ids(&patterns)), (1, vec![pending_pattern]));
    assert_eq!(
        (pending_of_first.total, ids(&pending_of_first)),
        (1, vec![pending_schema])
    );
    assert_eq!(
        (all_of_second.total, ids(&all_of_second)),
        (1, vec![other_pending])
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_search_reviews_by_shop_name_slug_host_and_review_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let antiques = insert_source(
        &pool,
        "Antik Sammler",
        "antik-sammler",
        "sammler.example.de",
        true,
    )
    .await;
    let percent = insert_source(&pool, "50% Shop", "fifty-shop", "fifty.example.com", true).await;
    let other = insert_source(&pool, "Other", "other", "other.example.com", true).await;
    let antiques_review = pending_schema_review(&repository, antiques).await;
    let percent_review = pending_schema_review(&repository, percent).await;
    let other_review = pending_schema_review(&repository, other).await;
    let search = |text: &str| ReviewQuery {
        search: SearchTerm::new(text),
        ..ReviewQuery::default()
    };

    let by_name = list_reviews(&repository, search("antik")).await;
    let by_slug = list_reviews(&repository, search("K-SAMM")).await;
    let by_host = list_reviews(&repository, search("sammler.example")).await;
    let by_id = list_reviews(&repository, search(&other_review.to_string())).await;
    let by_wildcard = list_reviews(&repository, search("50%")).await;
    let by_underscore = list_reviews(&repository, search("_")).await;
    let by_nothing = list_reviews(&repository, search("no such shop")).await;

    assert_eq!(review_ids(&by_name), vec![antiques_review]);
    assert_eq!(review_ids(&by_slug), vec![antiques_review]);
    assert_eq!(review_ids(&by_host), vec![antiques_review]);
    assert_eq!(review_ids(&by_id), vec![other_review]);
    assert_eq!(
        review_ids(&by_wildcard),
        vec![percent_review],
        "a % in the search matches itself, not any text"
    );
    assert_eq!(by_underscore.total, 0, "an _ in the search matches itself");
    assert_eq!(by_nothing.total, 0);
}

/// The `human_required` filter reads the agents' rounds as the review's own `human_required`
/// does, and only a pending review waits.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_the_reviews_that_wait_for_a_human() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let capped_source = insert_source(&pool, "Capped", "capped", "capped.example.com", true).await;
    let handed_source = insert_source(&pool, "Handed", "handed", "handed.example.com", true).await;
    let working_source =
        insert_source(&pool, "Working", "working", "working.example.com", true).await;
    let capped = pending_schema_review(&repository, capped_source).await;
    let handed = pending_schema_review(&repository, handed_source).await;
    let working = pending_schema_review(&repository, working_source).await;
    repository
        .request_repair(
            capped,
            ReviewActor::AgentReviewer,
            Some("the title is wrong"),
        )
        .await
        .unwrap();
    repository
        .hand_to_human(handed, ReviewActor::AgentAuthor, Some("needs a decision"))
        .await
        .unwrap();
    let closed = closed_schema_review(&repository, capped_source, "REJECTED").await;
    let waiting = |human_required| ReviewQuery {
        human_required: Some(human_required),
        ..ReviewQuery::default()
    };

    let waits = list_reviews(&repository, waiting(true)).await;
    let does_not_wait = list_reviews(&repository, waiting(false)).await;

    let mut waiting_ids = review_ids(&waits);
    waiting_ids.sort_by_key(|id| id.to_string());
    let mut expected = vec![capped, handed];
    expected.sort_by_key(|id| id.to_string());
    assert_eq!((waits.total, waiting_ids), (2, expected));
    assert!(waits.reviews.iter().all(|review| review.human_required));
    assert_eq!(does_not_wait.total, 2);
    assert!(review_ids(&does_not_wait).contains(&working));
    assert!(review_ids(&does_not_wait).contains(&closed));
    assert!(
        does_not_wait
            .reviews
            .iter()
            .all(|review| !review.human_required)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_wait_for_no_human_outside_agent_mode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let source = insert_source(&pool, "Shop", "shop", "shop.example.com", true).await;
    pending_schema_review(&repository, source).await;

    let waits = list_reviews(
        &repository,
        ReviewQuery {
            human_required: Some(true),
            ..ReviewQuery::default()
        },
    )
    .await;
    let counts = ConsoleOverviewReader::of(&repository)
        .overview()
        .await
        .unwrap();

    assert_eq!(waits.total, 0);
    assert_eq!(counts.pending_reviews.waiting_for_human, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_review_cursor_that_names_no_review() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());

    let result = ReviewQueueReader::of(&repository)
        .list_reviews(&ReviewQuery {
            after: Some(CrawlerReviewId::new()),
            ..ReviewQuery::default()
        })
        .await;

    assert!(matches!(result, Err(PageReadError::UnknownCursor(_))));
}

async fn list_sources(pool: &PgPool, query: ListingSourceQuery) -> ListingSourcePage {
    ListingSourceOverviewReader::new(pool.clone())
        .list_listing_sources(&query)
        .await
        .unwrap()
}

fn source_ids(page: &ListingSourcePage) -> Vec<ListingSourceId> {
    page.listing_sources
        .iter()
        .map(|source| source.listing_source_id)
        .collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_page_through_every_listing_source_in_the_order_asked_for() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let mut by_name = Vec::new();
    for (index, name) in ["Delta", "alpha", "Charlie", "bravo", "Echo"]
        .iter()
        .enumerate()
    {
        let id = insert_source(
            &pool,
            name,
            &format!("slug-{index}"),
            &format!("shop{index}.example.com"),
            true,
        )
        .await;
        by_name.push((name.to_lowercase(), id, index));
    }
    by_name.sort();
    // Two pending reviews for Echo, one for Charlie.
    let echo = by_name[4].1;
    let charlie = by_name[2].1;
    pending_schema_review(&repository, echo).await;
    pending_url_pattern_review(&repository, echo).await;
    pending_schema_review(&repository, charlie).await;

    let mut paged = Vec::new();
    let mut after = None;
    loop {
        let page = list_sources(
            &pool,
            ListingSourceQuery {
                sort: SourceSort::ListingSourceName,
                direction: SortDirection::Ascending,
                after,
                page_size: page_size(2),
                ..ListingSourceQuery::default()
            },
        )
        .await;
        assert_eq!(page.total, 5);
        paged.extend(source_ids(&page));
        match page.next_after {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    let by_reviews = list_sources(
        &pool,
        ListingSourceQuery {
            sort: SourceSort::PendingReviews,
            ..ListingSourceQuery::default()
        },
    )
    .await;

    assert_eq!(
        paged,
        by_name.iter().map(|(_, id, _)| *id).collect::<Vec<_>>(),
        "by name regardless of case, each source once"
    );
    assert_eq!(
        source_ids(&by_reviews)[..2],
        [echo, charlie],
        "the most pending reviews first"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_and_search_listing_sources_in_sql() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let healthy = insert_source(
        &pool,
        "Healthy Shop",
        "healthy",
        "healthy.example.com",
        true,
    )
    .await;
    let stopped = insert_source(
        &pool,
        "Stopped Shop",
        "stopped",
        "stopped.example.com",
        true,
    )
    .await;
    let failing = insert_source(
        &pool,
        "Failing Shop",
        "failing",
        "failing.example.com",
        true,
    )
    .await;
    let disabled = insert_source(
        &pool,
        "Disabled Shop",
        "disabled",
        "disabled.example.com",
        false,
    )
    .await;
    SpiderCandidateServiceImpl::new(pool.clone())
        .mark_crawl_needs_operator(
            &CrawledRoot::new(stopped, "https://stopped.example.com/".to_owned()),
            "RobotsBlocked",
            5,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_sources SET scrape_failure_streak = 3, last_scrape_error_kind = 'TIMEOUT' \
         WHERE listing_source_id = $1",
    )
    .bind(failing.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    pending_schema_review(&repository, disabled).await;
    let with = |health: Option<SourceHealth>, crawl_enabled: Option<bool>| ListingSourceQuery {
        health,
        crawl_enabled,
        ..ListingSourceQuery::default()
    };
    let sorted = |mut ids: Vec<ListingSourceId>| {
        ids.sort_by_key(|id| id.to_string());
        ids
    };

    let attention = list_sources(&pool, with(Some(SourceHealth::NeedsAttention), None)).await;
    let operator = list_sources(&pool, with(Some(SourceHealth::NeedsOperator), None)).await;
    let scrape_failing = list_sources(&pool, with(Some(SourceHealth::ScrapeFailing), None)).await;
    let healthy_page = list_sources(&pool, with(Some(SourceHealth::Healthy), None)).await;
    let not_crawled = list_sources(&pool, with(None, Some(false))).await;
    let by_host = list_sources(
        &pool,
        ListingSourceQuery {
            search: SearchTerm::new("STOPPED.example"),
            ..ListingSourceQuery::default()
        },
    )
    .await;
    let by_id = list_sources(
        &pool,
        ListingSourceQuery {
            search: SearchTerm::new(&failing.to_string()),
            ..ListingSourceQuery::default()
        },
    )
    .await;

    assert_eq!(
        (attention.total, sorted(source_ids(&attention))),
        (2, sorted(vec![stopped, failing]))
    );
    assert_eq!(source_ids(&operator), vec![stopped]);
    assert_eq!(source_ids(&scrape_failing), vec![failing]);
    assert_eq!(
        source_ids(&healthy_page),
        vec![healthy],
        "a source that is not crawled is neither healthy nor in need of attention"
    );
    assert_eq!(source_ids(&not_crawled), vec![disabled]);
    assert_eq!(source_ids(&by_host), vec![stopped]);
    assert_eq!(source_ids(&by_id), vec![failing]);
    assert!(
        attention
            .listing_sources
            .iter()
            .all(|source| source.needs_attention)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_continue_after_a_listing_source_that_no_longer_matches_the_filter() {
    let pool = get_postgres_client().await;
    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(
            insert_source(
                &pool,
                &format!("Shop {index}"),
                &format!("shop-{index}"),
                &format!("shop{index}.example.com"),
                true,
            )
            .await,
        );
    }
    let query = |after| ListingSourceQuery {
        crawl_enabled: Some(true),
        sort: SourceSort::ListingSourceName,
        direction: SortDirection::Ascending,
        after,
        page_size: page_size(2),
        ..ListingSourceQuery::default()
    };
    let first = list_sources(&pool, query(None)).await;
    assert_eq!(source_ids(&first), ids[..2]);
    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(ids[1].as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let second = list_sources(&pool, query(first.next_after)).await;

    assert_eq!(source_ids(&second), ids[2..]);
    assert_eq!(second.total, 3);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_listing_source_cursor_that_names_no_listing_source() {
    let pool = get_postgres_client().await;

    let result = ListingSourceOverviewReader::new(pool)
        .list_listing_sources(&ListingSourceQuery {
            after: Some(ListingSourceId::new()),
            ..ListingSourceQuery::default()
        })
        .await;

    assert!(matches!(result, Err(PageReadError::UnknownCursor(_))));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_the_listing_sources_the_crawler_works_on_right_now() {
    let pool = get_postgres_client().await;
    let crawled = insert_source(&pool, "Crawled", "crawled", "crawled.example.com", true).await;
    let scraped = insert_source(&pool, "Scraped", "scraped", "scraped.example.com", true).await;
    insert_source(&pool, "Idle", "idle", "idle.example.com", true).await;
    let work_locks = Arc::new(LocalLockManager::new());
    let reader =
        ListingSourceOverviewReader::new(pool.clone()).with_work_locks(Arc::clone(&work_locks));
    let _crawl = WorkLock::try_acquire(&work_locks, WorkLockScope::Spider, crawled).unwrap();
    let _scrape = WorkLock::try_acquire(&work_locks, WorkLockScope::Scraper, scraped).unwrap();
    let with = |activity| ListingSourceQuery {
        activity: Some(activity),
        ..ListingSourceQuery::default()
    };

    let crawling = reader
        .list_listing_sources(&with(SourceActivity::Crawling))
        .await
        .unwrap();
    let scraping = reader
        .list_listing_sources(&with(SourceActivity::Scraping))
        .await
        .unwrap();
    let active = reader
        .list_listing_sources(&with(SourceActivity::Active))
        .await
        .unwrap();
    let counts = ConsoleOverviewReader::of(&CrawlerReviewRepository::new(pool.clone()))
        .with_work_locks(Arc::clone(&work_locks))
        .overview()
        .await
        .unwrap();

    assert_eq!(source_ids(&crawling), vec![crawled]);
    assert_eq!(source_ids(&scraping), vec![scraped]);
    assert_eq!((active.total, active.listing_sources.len()), (2, 2));
    assert_eq!(
        (counts.sources.crawling_now, counts.sources.scraping_now),
        (1, 1)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_read_a_listing_source_with_its_review_history_url_pattern_and_schema() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let source = insert_source(&pool, "Antik", "antik", "antik.example.com", true).await;
    let other = insert_source(&pool, "Other", "other", "other.example.com", true).await;
    pending_schema_review(&repository, other).await;
    let oldest = closed_schema_review(&repository, source, "SUPERSEDED").await;
    let rejected = closed_schema_review(&repository, source, "REJECTED").await;
    let pattern = pending_url_pattern_review(&repository, source).await;
    let pending = pending_schema_review(&repository, source).await;
    sqlx::query(
        "UPDATE listing_sources SET url_pattern_state = 'MATCHED', url_pattern = '/item/\\d+', \
         url_pattern_exclude = '/item/0' WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_product_schemas \
         (listing_source_id, product_schema, created, updated) VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(source.as_uuid())
    .bind(serde_json::to_value(vec![schema()]).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let reader = ListingSourceDetailReader::new(
        ListingSourceOverviewReader::new(pool.clone()),
        ReviewQueueReader::of(&repository),
    );

    let detail = reader
        .find_listing_source(
            source,
            ReviewHistoryQuery {
                after: None,
                page_size: page_size(3),
            },
        )
        .await
        .unwrap()
        .expect("the source exists");
    let rest = reader
        .find_listing_source(
            source,
            ReviewHistoryQuery {
                after: detail.reviews.next_after,
                page_size: page_size(3),
            },
        )
        .await
        .unwrap()
        .expect("the source exists");

    assert_eq!(detail.listing_source.listing_source_id, source);
    assert_eq!(detail.listing_source.listing_source_slug, "antik");
    assert_eq!(
        (
            detail.url_pattern.state.as_str(),
            detail.url_pattern.pattern.as_deref(),
            detail.url_pattern.exclude.as_deref()
        ),
        ("MATCHED", Some("/item/\\d+"), Some("/item/0"))
    );
    assert_eq!(
        detail.product_schema.map(|schema| schema.schemas),
        Some(vec![schema()])
    );
    assert_eq!(detail.reviews.total, 4, "only the reviews of this source");
    assert_eq!(
        review_ids(&detail.reviews),
        vec![pending, pattern, rejected]
    );
    assert_eq!(review_ids(&rest.reviews), vec![oldest]);
    assert_eq!(rest.reviews.next_after, None);
    assert!(
        reader
            .find_listing_source(ListingSourceId::new(), ReviewHistoryQuery::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_read_a_listing_source_without_schema_as_having_none() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let source = insert_source(&pool, "Antik", "antik", "antik.example.com", true).await;
    let reader = ListingSourceDetailReader::new(
        ListingSourceOverviewReader::new(pool.clone()),
        ReviewQueueReader::of(&repository),
    );

    let detail = reader
        .find_listing_source(source, ReviewHistoryQuery::default())
        .await
        .unwrap()
        .expect("the source exists");

    assert!(detail.product_schema.is_none());
    assert_eq!(detail.url_pattern.state, "UNKNOWN");
    assert_eq!(detail.url_pattern.pattern, None);
    assert_eq!(detail.reviews.total, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_count_the_sources_and_pending_reviews_of_the_first_page() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let enabled = insert_source(&pool, "Enabled", "enabled", "enabled.example.com", true).await;
    let stopped = insert_source(&pool, "Stopped", "stopped", "stopped.example.com", true).await;
    let disabled =
        insert_source(&pool, "Disabled", "disabled", "disabled.example.com", false).await;
    SpiderCandidateServiceImpl::new(pool.clone())
        .mark_crawl_needs_operator(
            &CrawledRoot::new(stopped, "https://stopped.example.com/".to_owned()),
            "RobotsBlocked",
            5,
        )
        .await
        .unwrap();
    let waiting = pending_schema_review(&repository, enabled).await;
    pending_url_pattern_review(&repository, disabled).await;
    pending_schema_review(&repository, disabled).await;
    closed_schema_review(&repository, enabled, "APPROVED").await;
    repository
        .request_repair(
            waiting,
            ReviewActor::AgentReviewer,
            Some("the title is wrong"),
        )
        .await
        .unwrap();

    let counts = ConsoleOverviewReader::of(&repository)
        .overview()
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(counts).unwrap(),
        json!({
            "sources": {
                "total": 3,
                "crawl_enabled": 2,
                "needs_operator": 1,
                "crawling_now": 0,
                "scraping_now": 0
            },
            "pending_reviews": {
                "total": 3,
                "url_pattern": 1,
                "product_schema": 2,
                "waiting_for_human": 1
            }
        })
    );
}
