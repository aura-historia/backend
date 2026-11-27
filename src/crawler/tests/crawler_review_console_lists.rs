//! The review console's lists and counts as the readers answer them from SQL: the paginated review
//! queue and ListingSource overview with their filters, search and sort, one ListingSource's page,
//! and the counts of the first page. The query parsing and the HTTP answers are proven at their own
//! layers; these tests prove the SQL against a database.

use crawler::CrawlerReviewId;
use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::model::{
    ArtifactType, HandoverCategory, HandoverCause, HandoverReason, ListPage, ListWindow,
    ListingSourceOverview, PAGE_ROLE_PRIMARY, ReviewActor, ReviewQueueEntry, ReviewSort,
    ReviewStatus, ReviewWaiting, SchemaReviewPageInput, SearchTerm, SortDirection, SourceActivity,
    SourceCrawl, SourceEventKind, SourceLabel, SourceLabelRequest, SourceSort, SourceStatus,
    UrlPatternDecision,
};
use crawler::review::readers::{
    ConsoleOverviewReader, HeldRemovalReader, ListingSourceDetailReader,
    ListingSourceOverviewReader, ListingSourceQuery, ReviewQuery, ReviewQueueReader,
    SourceEventReader,
};
use crawler::review::repository::{CrawlerReviewRepository, SchemaReviewWithStatusInput};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::service::activity_history::{
    ActivityCounts, ActivityHistoryRepository, CrawlRunClass,
};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::spider::crawled_root::CrawledRoot;
use crawler::spider::local_lock::{LocalLockManager, WorkLock, WorkLockScope};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use std::num::NonZeroU32;
use std::sync::Arc;
use test_api::*;
use time::OffsetDateTime;

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
        title: Some(rule("h1")),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: Some(rule("span.state")),
        images: Some(ExtractionRule {
            selector: "img.product".into(),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        }),
        raw_attributes: Default::default(),
        sanitize: Default::default(),
    }
}

fn review_pages() -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: "https://example.com/products/1".to_string(),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: "<html><body><h1>Title</h1></body></html>".to_string(),
    }]
}

fn window(limit: usize, offset: usize) -> ListWindow {
    ListWindow::new(limit, offset).unwrap_or_else(|| panic!("{limit} is a limit"))
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

async fn list_reviews(
    repository: &CrawlerReviewRepository,
    query: ReviewQuery,
) -> ListPage<ReviewQueueEntry> {
    ReviewQueueReader::of(repository)
        .list_reviews(&query)
        .await
        .unwrap()
}

fn review_ids(page: &ListPage<ReviewQueueEntry>) -> Vec<CrawlerReviewId> {
    page.items.iter().map(|review| review.review_id).collect()
}

fn sorted<T: ToString + Copy>(ids: &[T]) -> Vec<String> {
    let mut ids = ids.iter().map(ToString::to_string).collect::<Vec<_>>();
    ids.sort();
    ids
}

/// A ListingSource whose name, host and enabled flag the test chooses, without a slug that matters.
async fn source(pool: &PgPool, name: &str, host: &str, enabled: bool) -> ListingSourceId {
    insert_source(
        pool,
        name,
        &name.to_lowercase().replace(' ', "-"),
        host,
        enabled,
    )
    .await
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_read_every_review_in_windows_of_the_queue_order() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let enabled = source(&pool, "Enabled", "enabled.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", false).await;
    let closed_old = closed_schema_review(&repository, enabled, "REJECTED").await;
    let pending_disabled = pending_schema_review(&repository, disabled).await;
    let pending_old = pending_url_pattern_review(&repository, enabled).await;
    let pending_new = pending_schema_review(&repository, enabled).await;
    let closed_new = closed_schema_review(&repository, enabled, "APPROVED").await;

    let all = list_reviews(&repository, ReviewQuery::default()).await;
    let second = list_reviews(
        &repository,
        ReviewQuery {
            window: window(2, 2),
            ..ReviewQuery::default()
        },
    )
    .await;

    assert_eq!(
        review_ids(&all),
        vec![
            pending_old,
            pending_new,
            pending_disabled,
            closed_new,
            closed_old
        ],
        "pending of enabled sources oldest first, pending of disabled ones, then closed newest first"
    );
    assert_eq!((all.total, all.limit, all.offset), (5, 100, 0));
    assert_eq!(review_ids(&second), vec![pending_disabled, closed_new]);
    assert_eq!((second.total, second.limit, second.offset), (5, 2, 2));
    let beyond = list_reviews(
        &repository,
        ReviewQuery {
            window: window(2, 10),
            ..ReviewQuery::default()
        },
    )
    .await;
    assert_eq!((beyond.items.len(), beyond.total), (0, 5));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_sort_the_review_queue_by_the_column_asked_for() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let zeta = source(&pool, "Zeta", "zeta.example.com", true).await;
    let alpha = source(&pool, "Alpha", "alpha.example.com", true).await;
    let first = pending_schema_review(&repository, zeta).await;
    let second = pending_schema_review(&repository, alpha).await;
    sqlx::query(
        "UPDATE crawler_reviews SET updated = NOW() + INTERVAL '1 hour' WHERE review_id = $1",
    )
    .bind(first.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let sorted_by = |sort, direction| ReviewQuery {
        sort,
        direction,
        ..ReviewQuery::default()
    };

    let newest = list_reviews(
        &repository,
        sorted_by(ReviewSort::Created, SortDirection::Descending),
    )
    .await;
    let oldest = list_reviews(
        &repository,
        sorted_by(ReviewSort::Created, SortDirection::Ascending),
    )
    .await;
    let by_name = list_reviews(
        &repository,
        sorted_by(ReviewSort::Source, SortDirection::Ascending),
    )
    .await;
    let by_update = list_reviews(
        &repository,
        sorted_by(ReviewSort::Updated, SortDirection::Descending),
    )
    .await;

    assert_eq!(review_ids(&newest), vec![second, first]);
    assert_eq!(review_ids(&oldest), vec![first, second]);
    assert_eq!(review_ids(&by_name), vec![second, first]);
    assert_eq!(review_ids(&by_update), vec![first, second]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_reviews_by_status_artifact_type_and_listing_source() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let first = source(&pool, "First", "first.example.com", true).await;
    let second = source(&pool, "Second", "second.example.com", true).await;
    let approved = closed_schema_review(&repository, first, "APPROVED").await;
    closed_schema_review(&repository, first, "REJECTED").await;
    let pending_schema = pending_schema_review(&repository, first).await;
    let pending_pattern = pending_url_pattern_review(&repository, first).await;
    let other_pending = pending_schema_review(&repository, second).await;

    let by_status = list_reviews(
        &repository,
        ReviewQuery {
            status: Some(ReviewStatus::Approved),
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
            status: Some(ReviewStatus::PendingReview),
            artifact_type: Some(ArtifactType::ProductSchema),
            ..ReviewQuery::default()
        },
    )
    .await;
    let of_second = list_reviews(
        &repository,
        ReviewQuery {
            listing_source_id: Some(second),
            ..ReviewQuery::default()
        },
    )
    .await;

    assert_eq!(review_ids(&by_status), vec![approved]);
    assert_eq!(review_ids(&patterns), vec![pending_pattern]);
    assert_eq!(review_ids(&pending_of_first), vec![pending_schema]);
    assert_eq!(review_ids(&of_second), vec![other_pending]);
    assert_eq!(by_status.total, 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_search_reviews_by_listing_source_name_and_match_wildcards_literally() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let antiques = source(&pool, "Antik Sammler", "sammler.example.de", true).await;
    let percent = source(&pool, "50% Shop", "fifty.example.com", true).await;
    let antiques_review = pending_schema_review(&repository, antiques).await;
    let percent_review = pending_schema_review(&repository, percent).await;
    let search = |text: &str| ReviewQuery {
        search: SearchTerm::new(text),
        ..ReviewQuery::default()
    };

    let by_name = list_reviews(&repository, search("ANTIK")).await;
    let by_wildcard = list_reviews(&repository, search("50%")).await;
    let by_underscore = list_reviews(&repository, search("_")).await;

    assert_eq!(review_ids(&by_name), vec![antiques_review]);
    assert_eq!(review_ids(&by_wildcard), vec![percent_review]);
    assert_eq!(by_underscore.total, 0);
}

/// The `waiting` filter reads the agents' rounds as the review's own `human_required` does: a
/// pending review waits for a human at the cap or after a hand-over, and for a repair while a
/// needs-repair verdict is open below the cap.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_the_reviews_that_wait_for_a_human_or_a_repair() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 2);
    let capped_source = source(&pool, "Capped", "capped.example.com", true).await;
    let handed_source = source(&pool, "Handed", "handed.example.com", true).await;
    let repair_source = source(&pool, "Repair", "repair.example.com", true).await;
    let working_source = source(&pool, "Working", "working.example.com", true).await;
    let capped = pending_schema_review(&repository, capped_source).await;
    let handed = pending_schema_review(&repository, handed_source).await;
    let repair = pending_schema_review(&repository, repair_source).await;
    let working = pending_schema_review(&repository, working_source).await;
    for _ in 0..2 {
        repository
            .request_repair(
                capped,
                ReviewActor::AgentReviewer,
                Some("the title is wrong"),
            )
            .await
            .unwrap();
    }
    repository
        .request_repair(
            repair,
            ReviewActor::AgentReviewer,
            Some("the price is wrong"),
        )
        .await
        .unwrap();
    repository
        .hand_to_human(
            handed,
            ReviewActor::AgentAuthor,
            HandoverCause {
                reason: HandoverReason::NeedsHuman,
                category: HandoverCategory::PagesUnreadable,
            },
            Some("needs a decision"),
        )
        .await
        .unwrap();
    let waiting = |waiting| ReviewQuery {
        waiting: Some(waiting),
        ..ReviewQuery::default()
    };

    let human = list_reviews(&repository, waiting(ReviewWaiting::Human)).await;
    let repairing = list_reviews(&repository, waiting(ReviewWaiting::Repair)).await;
    let everything = list_reviews(&repository, ReviewQuery::default()).await;

    assert_eq!(sorted(&review_ids(&human)), sorted(&[capped, handed]));
    assert!(human.items.iter().all(|review| review.human_required));
    assert_eq!(review_ids(&repairing), vec![repair]);
    assert_eq!(
        everything
            .items
            .iter()
            .filter(|review| review.human_required)
            .count(),
        2
    );
    assert!(review_ids(&everything).contains(&working));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_wait_for_no_human_outside_agent_mode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let shop = source(&pool, "Shop", "shop.example.com", true).await;
    pending_schema_review(&repository, shop).await;

    let waits = list_reviews(
        &repository,
        ReviewQuery {
            waiting: Some(ReviewWaiting::Human),
            ..ReviewQuery::default()
        },
    )
    .await;
    let counts = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap();

    assert_eq!(waits.total, 0);
    assert_eq!(counts.reviews.waiting_for_human, 0);
    assert!(!counts.agent_mode);
}

/// "Waiting for a human" counts and lists only the reviews of crawled ListingSources, so the tile
/// and the list it links to agree and a review nobody can act on is not counted (#3006). The
/// reviews of sources the agents parked, by a label that expires or by an undecided qualification,
/// form the Parked group instead, with the label and the time of the next retry; a disabled source
/// the agents did not park is in neither.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_count_only_crawled_reviews_as_waiting_for_a_human_and_group_the_parked_ones() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let crawled = source(&pool, "Crawled", "crawled.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", true).await;
    let parked_label = source(&pool, "Challenged", "challenged.example.com", true).await;
    let parked_qualification = source(&pool, "Unreachable", "unreachable.example.com", true).await;
    let mut reviews = Vec::new();
    for listing_source_id in [crawled, disabled, parked_label, parked_qualification] {
        let review_id = pending_schema_review(&repository, listing_source_id).await;
        repository
            .request_repair(review_id, ReviewActor::AgentReviewer, Some("still wrong"))
            .await
            .unwrap();
        reviews.push(review_id);
    }
    for (listing_source_id, park) in [
        (disabled, "crawl_enabled = FALSE"),
        (
            parked_label,
            "crawl_enabled = FALSE, source_label = 'BOT_PROTECTION', \
             source_label_note = 'challenge on every item page', source_labelled_at = NOW(), \
             source_labelled_by = 'AGENT_AUTHOR', \
             source_label_retry_at = NOW() + INTERVAL '30 days'",
        ),
        (
            parked_qualification,
            "crawl_enabled = FALSE, qualification = 'NEEDS_HUMAN', \
             qualification_note = 'unreachable', qualified_at = NOW(), \
             qualified_by = 'AGENT_AUTHOR', qualification_retry_at = NOW() + INTERVAL '60 days'",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE listing_sources SET {park} WHERE listing_source_id = $1"
        )))
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }
    let query = |waiting, label| ReviewQuery {
        waiting: Some(waiting),
        label,
        ..ReviewQuery::default()
    };

    let human = list_reviews(&repository, query(ReviewWaiting::Human, None)).await;
    let parked = list_reviews(&repository, query(ReviewWaiting::Parked, None)).await;
    let challenged = list_reviews(
        &repository,
        query(ReviewWaiting::Parked, Some(SourceLabel::BotProtection)),
    )
    .await;
    let counts = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap();
    let parked_sources = list_sources(
        &pool,
        ListingSourceQuery {
            status: Some(SourceStatus::Parked),
            ..ListingSourceQuery::default()
        },
    )
    .await;

    assert_eq!(review_ids(&human), vec![reviews[0]]);
    assert_eq!(counts.reviews.waiting_for_human, 1);
    assert_eq!(
        sorted(&review_ids(&parked)),
        sorted(&[reviews[2], reviews[3]])
    );
    let park_of = |review_id| {
        parked
            .items
            .iter()
            .find(|review| review.review_id == review_id)
            .and_then(|review| review.parked.clone())
            .unwrap_or_else(|| panic!("{review_id} is parked"))
    };
    assert_eq!(park_of(reviews[2]).label, Some(SourceLabel::BotProtection));
    assert_eq!(park_of(reviews[3]).label, None);
    assert!(park_of(reviews[2]).next_retry_at < park_of(reviews[3]).next_retry_at);
    assert_eq!(review_ids(&challenged), vec![reviews[2]]);
    assert_eq!((counts.reviews.parked, counts.sources.parked), (2, 2));
    assert_eq!(
        sorted(&source_ids(&parked_sources)),
        sorted(&[parked_label, parked_qualification])
    );
}

fn overview_reader(pool: &PgPool, repository: &CrawlerReviewRepository) -> ConsoleOverviewReader {
    ConsoleOverviewReader::new(repository, ListingSourceOverviewReader::new(pool.clone()))
}

async fn list_sources(pool: &PgPool, query: ListingSourceQuery) -> ListPage<ListingSourceOverview> {
    ListingSourceOverviewReader::new(pool.clone())
        .list_listing_sources(&query)
        .await
        .unwrap()
}

fn source_ids(page: &ListPage<ListingSourceOverview>) -> Vec<ListingSourceId> {
    page.items
        .iter()
        .map(|source| source.listing_source_id)
        .collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_read_listing_sources_in_windows_of_the_order_asked_for() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let mut by_name = Vec::new();
    for (index, name) in ["Delta", "alpha", "Charlie", "bravo", "Echo"]
        .iter()
        .enumerate()
    {
        let id = source(&pool, name, &format!("shop{index}.example.com"), true).await;
        by_name.push((name.to_lowercase(), id));
    }
    by_name.sort();
    let echo = by_name[4].1;
    let charlie = by_name[2].1;
    pending_schema_review(&repository, echo).await;
    pending_url_pattern_review(&repository, echo).await;
    pending_schema_review(&repository, charlie).await;
    let named = |offset| ListingSourceQuery {
        sort: SourceSort::Name,
        direction: SortDirection::Ascending,
        window: window(2, offset),
        ..ListingSourceQuery::default()
    };

    let mut paged = Vec::new();
    for offset in [0, 2, 4] {
        let page = list_sources(&pool, named(offset)).await;
        assert_eq!((page.total, page.limit, page.offset), (5, 2, offset));
        paged.extend(source_ids(&page));
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
        by_name.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
        "by name regardless of case, each source once"
    );
    assert_eq!(source_ids(&by_reviews)[..2], [echo, charlie]);
}

/// The attention order keeps the disabled sources of each console group together: those whose
/// qualification waits for a human, then those that wait for their qualification, then the
/// others, whatever their pending reviews.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_rank_disabled_sources_by_their_qualification_for_attention() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let disabled = source(&pool, "Out of scope", "out.example.com", false).await;
    let awaiting = source(&pool, "Awaiting", "awaiting.example.com", false).await;
    let needs_human = source(&pool, "Unclear", "unclear.example.com", false).await;
    pending_schema_review(&repository, disabled).await;
    for (id, set) in [
        (awaiting, "in_crawl_scope = TRUE"),
        (
            needs_human,
            "in_crawl_scope = TRUE, qualification = 'NEEDS_HUMAN', \
             qualification_note = 'every page answers a bot challenge', qualified_at = NOW(), \
             qualified_by = 'AGENT_AUTHOR'",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE listing_sources SET {set} WHERE listing_source_id = $1"
        )))
        .bind(id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    let ranked = list_sources(&pool, ListingSourceQuery::default()).await;

    assert_eq!(source_ids(&ranked), vec![needs_human, awaiting, disabled]);
}

/// The attention order keeps each status the console groups the table by in one run of rows: a
/// source failing to scrape that is also blocked shows as failing, so it ranks with the failing
/// ones before every blocked one, and the never-scraped sources come before the healthy ones
/// whatever their pending reviews.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_rank_crawl_enabled_sources_in_the_groups_the_console_shows() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let failing_and_blocked = source(&pool, "Failing and blocked", "both.example.com", true).await;
    let failing = source(&pool, "Failing", "failing.example.com", true).await;
    let blocked = source(&pool, "Blocked", "blocked.example.com", true).await;
    let healthy = source(&pool, "Healthy", "healthy.example.com", true).await;
    let never_scraped = source(&pool, "Never scraped", "never.example.com", true).await;
    for (listing_source_id, host, scraped) in [
        (failing_and_blocked, "both.example.com", false),
        (blocked, "blocked.example.com", false),
        (healthy, "healthy.example.com", true),
    ] {
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, url, url_class, raw_source_record_key, last_scraped) \
             VALUES ($1, $2, 'product', $2, CASE WHEN $3 THEN NOW() END)",
        )
        .bind(listing_source_id.as_uuid())
        .bind(format!("https://{host}/item/1"))
        .bind(scraped)
        .execute(&pool)
        .await
        .unwrap();
    }
    for listing_source_id in [failing_and_blocked, blocked] {
        pending_schema_review(&repository, listing_source_id).await;
    }
    // URL-pattern reviews block no URL; they only rank the healthy source by its pending reviews.
    for _ in 0..2 {
        pending_url_pattern_review(&repository, healthy).await;
    }
    for listing_source_id in [failing_and_blocked, failing] {
        sqlx::query(
            "UPDATE listing_sources SET scrape_failure_streak = 2, last_scrape_error_kind = 'TIMEOUT' \
             WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    let ranked = list_sources(&pool, ListingSourceQuery::default()).await;

    let ids = source_ids(&ranked);
    let position = |id| ids.iter().position(|ranked| *ranked == id).unwrap();
    assert!(position(failing_and_blocked) < position(blocked));
    assert!(position(failing) < position(blocked), "{ids:?}");
    assert!(
        position(never_scraped) < position(healthy),
        "a healthy source with more pending reviews must not split the never-scraped group: {ids:?}"
    );
    assert_eq!(position(healthy), ids.len() - 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_and_search_listing_sources_in_sql() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let healthy = source(&pool, "Healthy Shop", "healthy.example.com", true).await;
    let stopped = source(&pool, "Stopped Shop", "stopped.example.com", true).await;
    let failing = source(&pool, "Failing Shop", "failing.example.com", true).await;
    let disabled = source(&pool, "Disabled Shop", "disabled.example.com", false).await;
    let labelled = source(&pool, "Gallery", "gallery.example.com", true).await;
    for (root, listing_source_id) in [
        ("https://stopped.example.com/", stopped),
        ("https://gallery.example.com/", labelled),
    ] {
        SpiderCandidateServiceImpl::new(pool.clone())
            .mark_crawl_needs_operator(
                &CrawledRoot::new(listing_source_id, root.to_owned()),
                "RobotsBlocked",
                5,
            )
            .await
            .unwrap();
    }
    repository
        .label_source(
            &labelled,
            ReviewActor::Human,
            &SourceLabelRequest::new(SourceLabel::NotAnEshop, None).unwrap(),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_sources SET scrape_failure_streak = 3, last_scrape_error_kind = 'TIMEOUT'          WHERE listing_source_id = $1",
    )
    .bind(failing.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    pending_schema_review(&repository, disabled).await;
    let with = |status| ListingSourceQuery {
        status: Some(status),
        ..ListingSourceQuery::default()
    };

    let attention = list_sources(&pool, with(SourceStatus::NeedsAttention)).await;
    let operator = list_sources(&pool, with(SourceStatus::CrawlNeedsOperator)).await;
    let failing_page = list_sources(&pool, with(SourceStatus::Failing)).await;
    let healthy_page = list_sources(&pool, with(SourceStatus::Healthy)).await;
    let disabled_page = list_sources(&pool, with(SourceStatus::Disabled)).await;
    let labelled_page = list_sources(&pool, with(SourceStatus::Labelled)).await;
    let by_name = list_sources(
        &pool,
        ListingSourceQuery {
            search: SearchTerm::new("STOPPED SH"),
            ..ListingSourceQuery::default()
        },
    )
    .await;
    let by_host = list_sources(
        &pool,
        ListingSourceQuery {
            search: SearchTerm::new("failing.example"),
            ..ListingSourceQuery::default()
        },
    )
    .await;

    assert_eq!(
        sorted(&source_ids(&attention)),
        sorted(&[stopped, failing]),
        "a labelled source never needs attention, whatever its crawl left"
    );
    assert_eq!(source_ids(&operator), vec![stopped]);
    assert_eq!(source_ids(&failing_page), vec![failing]);
    assert_eq!(
        source_ids(&healthy_page),
        vec![healthy],
        "a source that is not crawled is neither healthy nor in need of attention"
    );
    assert_eq!(
        source_ids(&disabled_page),
        vec![disabled],
        "labelled is not disabled"
    );
    assert_eq!(source_ids(&labelled_page), vec![labelled]);
    assert_eq!(
        labelled_page.items[0]
            .source_label
            .as_ref()
            .map(|applied| applied.label),
        Some(SourceLabel::NotAnEshop)
    );
    assert_eq!(source_ids(&by_name), vec![stopped]);
    assert_eq!(source_ids(&by_host), vec![failing]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_the_listing_sources_the_crawler_works_on_right_now() {
    let pool = get_postgres_client().await;
    let crawled = source(&pool, "Crawled", "crawled.example.com", true).await;
    let scraped = source(&pool, "Scraped", "scraped.example.com", true).await;
    let idle = source(&pool, "Idle", "idle.example.com", true).await;
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
    let idling = reader
        .list_listing_sources(&with(SourceActivity::Idle))
        .await
        .unwrap();
    let counts = ConsoleOverviewReader::new(&CrawlerReviewRepository::new(pool.clone()), reader)
        .overview()
        .await
        .unwrap();

    assert_eq!(source_ids(&crawling), vec![crawled]);
    assert_eq!(source_ids(&scraping), vec![scraped]);
    assert_eq!(source_ids(&idling), vec![idle]);
    assert_eq!(
        (counts.sources.crawling_now, counts.sources.scraping_now),
        (1, 1)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_read_a_listing_source_with_its_live_configuration() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let shop = source(&pool, "Antik", "antik.example.com", true).await;
    let pattern_review = pending_url_pattern_review(&repository, shop).await;
    repository
        .reject_review(pattern_review, ReviewActor::Human, Some("no"), None)
        .await
        .unwrap();
    let approved_schema = closed_schema_review(&repository, shop, "APPROVED").await;
    sqlx::query(
        "UPDATE listing_sources SET url_pattern_state = 'MATCHED', url_pattern_matching = 'FULL_URL_SEARCH', url_pattern = '/item/\\d+',          url_pattern_exclude = '/item/0' WHERE listing_source_id = $1",
    )
    .bind(shop.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    // The product schema is stored in the single-object shape an older version wrote.
    sqlx::query(
        "INSERT INTO listing_source_product_schemas          (listing_source_id, product_schema, created, updated) VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(shop.as_uuid())
    .bind(serde_json::to_value(schema()).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let reader = ListingSourceDetailReader::new(ListingSourceOverviewReader::new(pool.clone()));

    let detail = reader
        .find_listing_source(shop)
        .await
        .unwrap()
        .expect("the source exists");
    let json = serde_json::to_value(&detail).unwrap();

    assert_eq!(json["listing_source_id"], shop.to_string());
    assert_eq!(json["url_pattern"]["value"], "/item/\\d+");
    assert_eq!(json["url_pattern"]["exclude"], "/item/0");
    assert_eq!(json["url_pattern"]["matching"], "FULL_URL_SEARCH");
    assert_eq!(
        json["url_pattern"]["approved_review_id"],
        json!(null),
        "no approved pattern review"
    );
    assert_eq!(detail.product_schemas, vec![schema()]);
    assert_eq!(detail.removed_page_schemas, vec![]);
    assert_eq!(detail.live_schema_review_id, Some(approved_schema));
    assert!(detail.live_schemas_since.is_some());
    assert!(
        reader
            .find_listing_source(ListingSourceId::new())
            .await
            .unwrap()
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_name_who_approved_the_live_url_pattern() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let shop = source(&pool, "Antik", "antik.example.com", true).await;
    sqlx::query(
        "UPDATE listing_sources SET url_pattern_state = 'MATCHED', url_pattern_matching = 'FULL_URL_SEARCH', url_pattern = '/item/'          WHERE listing_source_id = $1",
    )
    .bind(shop.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let review = pending_url_pattern_review(&repository, shop).await;
    sqlx::query(
        "UPDATE crawler_reviews SET status = 'APPROVED', reviewed = NOW() WHERE review_id = $1",
    )
    .bind(review.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_events (event_id, review_id, actor, action, candidate_version, details)          VALUES (gen_random_uuid(), $1, 'AGENT_REVIEWER', 'APPROVED', 1, '{}'::jsonb)",
    )
    .bind(review.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let reader = ListingSourceDetailReader::new(ListingSourceOverviewReader::new(pool.clone()));

    let json = serde_json::to_value(
        reader
            .find_listing_source(shop)
            .await
            .unwrap()
            .expect("the source exists"),
    )
    .unwrap();

    assert_eq!(
        json["url_pattern"]["approved_review_id"],
        review.to_string()
    );
    assert_eq!(json["url_pattern"]["approved_by"], "AGENT_REVIEWER");
    assert!(json["url_pattern"]["approved_at"].is_string());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_the_reviews_of_a_listing_source_with_their_live_flag_and_last_actor() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let shop = source(&pool, "Antik", "antik.example.com", true).await;
    let other = source(&pool, "Other", "other.example.com", true).await;
    pending_schema_review(&repository, other).await;
    let old_approved = closed_schema_review(&repository, shop, "APPROVED").await;
    let live_approved = closed_schema_review(&repository, shop, "APPROVED").await;
    let pending = pending_schema_review(&repository, shop).await;
    repository
        .request_repair(
            pending,
            ReviewActor::AgentReviewer,
            Some("the title is wrong"),
        )
        .await
        .unwrap();
    let reader = ReviewQueueReader::of(&repository);

    let all = reader
        .list_source_reviews(shop, &ReviewQuery::default())
        .await
        .unwrap();
    let approved_only = reader
        .list_source_reviews(
            shop,
            &ReviewQuery {
                status: Some(ReviewStatus::Approved),
                window: window(1, 1),
                ..ReviewQuery::default()
            },
        )
        .await
        .unwrap();

    let flags = all
        .items
        .iter()
        .map(|entry| {
            (
                entry.review.review_id,
                entry.candidate_is_live,
                entry.last_actor,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(all.total, 3, "only this source's reviews, newest first");
    assert_eq!(
        flags,
        vec![
            (pending, false, Some(ReviewActor::AgentReviewer)),
            (live_approved, true, None),
            (old_approved, false, None),
        ]
    );
    assert!(all.items.iter().all(|entry| !entry.rejection_revoked));
    assert_eq!(approved_only.total, 2);
    assert_eq!(approved_only.items[0].review.review_id, old_approved);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_count_the_sources_and_pending_reviews_of_the_first_page() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let enabled = source(&pool, "Enabled", "enabled.example.com", true).await;
    let stopped = source(&pool, "Stopped", "stopped.example.com", true).await;
    let blocked = source(&pool, "Blocked", "blocked.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", false).await;
    SpiderCandidateServiceImpl::new(pool.clone())
        .mark_crawl_needs_operator(
            &CrawledRoot::new(stopped, "https://stopped.example.com/".to_owned()),
            "RobotsBlocked",
            5,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_sources SET scrape_failure_streak = 2, last_scrape_error_kind = 'TIMEOUT'          WHERE listing_source_id = $1",
    )
    .bind(blocked.as_uuid())
    .execute(&pool)
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

    let counts = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap();

    let mut counts = serde_json::to_value(counts).unwrap();
    let freshness = counts["freshness"].take();
    let crawl_runs_by_hour = counts["throughput"]["crawl_runs_by_hour"].take();
    let daily_review_flow = counts["review_flow"]["daily"].take();
    let activity = counts["activity"].take();
    let oldest_pending = counts["reviews"]["oldest_pending"].take();
    let median_pending_age = counts["reviews"]["median_pending_age_secs"].take();
    assert_eq!(
        counts,
        json!({
            "sources": {
                "crawling_now": 0, "scraping_now": 0, "needs_attention": 2,
                "crawl_failing": 0, "crawl_needs_operator": 1, "blocked_by_review": 0,
                "failing_to_scrape": 1, "held_removals": 0, "identity_conflicts": 0,
                "labelled": 0, "parked": 0, "stale": 0, "rate_limited": 0, "scrape_paused": 0
            },
            "freshness": null,
            "throughput": {
                "pages_scraped_1h": 0, "pages_scraped_24h": 0, "product_urls_found_24h": 0,
                "crawls_finished_1h": 1, "crawls_finished_24h": 1, "crawl_runs_by_hour": null
            },
            "crawls": {
                "total": 3, "crawled_within_a_day": 1, "due": 0, "overdue": 0,
                "never_crawled": 2
            },
            "pages": {
                "total": 0, "scraped_within_a_day": 0, "due": 0, "overdue": 0,
                "never_scraped": 0
            },
            "failures": {
                "scrape": [{ "error_kind": "TIMEOUT", "sources": 1 }],
                "crawl": []
            },
            "reviews": {
                "pending_total": 3,
                "pending_by_artifact": { "PRODUCT_SCHEMA": 2, "URL_PATTERN": 1 },
                "pending_disabled_sources": 2,
                "waiting_for_human": 1,
                "parked": 0,
                "held_for_non_product_loop": 0,
                "oldest_pending": null,
                "median_pending_age_secs": null
            },
            "review_flow": { "daily": null, "median_decision_secs_7d": 0 },
            "activity": null,
            "agent_mode": true
        })
    );
    assert!(
        activity["recording_since"].is_string(),
        "the hour the migration ran in: {activity}"
    );
    assert_eq!(activity["by_hour"].as_array().map(Vec::len), Some(24));
    assert_eq!(freshness["since_last_scrape"], json!(null));
    assert_eq!(freshness["since_last_crawl"]["sources"], 1);
    assert_eq!(freshness["longest_idle"], json!([]));
    assert_eq!(
        crawl_runs_by_hour
            .as_array()
            .unwrap()
            .iter()
            .map(|hour| hour["sources"].as_i64().unwrap())
            .sum::<i64>(),
        1,
        "the stopped crawl: {crawl_runs_by_hour}"
    );
    assert_eq!(
        daily_review_flow[6],
        json!({ "opened": 4, "approved": 1, "rejected": 0 }),
        "the last 24 hours: {daily_review_flow}"
    );
    assert!(oldest_pending.is_string(), "{oldest_pending}");
    assert!(median_pending_age.is_i64(), "{median_pending_age}");
    let crawled_pending = list_reviews(
        &repository,
        ReviewQuery {
            status: Some(ReviewStatus::PendingReview),
            source_crawl: Some(SourceCrawl::Enabled),
            ..ReviewQuery::default()
        },
    )
    .await;
    assert_eq!(
        review_ids(&crawled_pending),
        vec![waiting],
        "the pending reviews the header and the Overview count: of crawl-enabled sources"
    );
}

/// The clock hour (UTC) `at` falls in.
fn clock_hour(at: OffsetDateTime) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(at.unix_timestamp().div_euclid(3600) * 3600)
        .unwrap_or_else(|error| panic!("{error}"))
}

/// What the scraper and the spider add to the activity history, summed per UTC clock hour, as the
/// Overview reads the last day of it; an hour past the retention is deleted by the first write of
/// a later hour.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_sum_the_activity_added_in_each_clock_hour_and_forget_expired_hours() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let history = ActivityHistoryRepository::new(pool.clone());
    let this_hour = clock_hour(OffsetDateTime::now_utc());
    let last_hour = this_hour - time::Duration::hours(1);
    let add = |at: OffsetDateTime, counts: ActivityCounts| {
        let history = &history;
        async move { history.add(at, counts).await.unwrap() }
    };

    add(
        this_hour - time::Duration::days(40),
        ActivityCounts::scrape_batch(9, 9),
    )
    .await;
    add(this_hour, ActivityCounts::scrape_batch(10, 2)).await;
    add(
        this_hour + time::Duration::seconds(1),
        ActivityCounts::scrape_batch(5, 0),
    )
    .await;
    add(
        last_hour,
        ActivityCounts::crawl_run(CrawlRunClass::Completed, 4),
    )
    .await;
    add(
        last_hour + time::Duration::minutes(59) + time::Duration::seconds(59),
        ActivityCounts::crawl_run(CrawlRunClass::Failed, 0),
    )
    .await;
    add(
        last_hour,
        ActivityCounts::crawl_run(CrawlRunClass::Other, 1),
    )
    .await;

    let activity = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap()
        .activity;

    let hour = |at: OffsetDateTime| {
        activity
            .by_hour
            .iter()
            .find(|entry| entry.hour == at)
            .copied()
            .unwrap_or_else(|| panic!("{at} is among the last 24 hours: {activity:?}"))
    };
    let now = hour(this_hour);
    assert_eq!(
        (now.pages_scraped, now.page_scrapes_failed),
        (15, 2),
        "two batches of this hour: {now:?}"
    );
    let before = hour(last_hour);
    assert_eq!(
        (
            before.crawl_runs_completed,
            before.crawl_runs_failed,
            before.crawl_runs_other,
            before.product_urls_found,
            before.pages_scraped
        ),
        (1, 1, 1, 5, 0),
        "three runs of the hour before, one in its last second: {before:?}"
    );
    assert_eq!(
        activity
            .by_hour
            .iter()
            .map(|entry| entry.pages_scraped)
            .sum::<i64>(),
        15
    );
    assert_eq!(
        activity
            .by_hour
            .windows(2)
            .map(|pair| pair[1].hour - pair[0].hour)
            .collect::<Vec<_>>(),
        vec![time::Duration::hours(1); 23],
        "24 consecutive clock hours, the oldest first"
    );
    let expired: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crawler_activity_hours WHERE hour < NOW() - INTERVAL '35 days'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(expired, 0, "the hour 40 days ago was deleted");
    assert!(
        activity
            .recording_since
            .is_some_and(|since| since > this_hour - time::Duration::days(35)),
        "the earliest hour kept, the expired one gone: {:?}",
        activity.recording_since
    );
}

/// The Overview's freshness, throughput and page freshness over the crawl-enabled sources, and
/// its stale count, which the `stale` list shows longest idle first as `longest_idle` names them.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_report_freshness_throughput_page_freshness_and_the_stale_sources() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let fresh = source(&pool, "Fresh", "fresh.example.com", true).await;
    let idle = source(&pool, "Idle", "idle.example.com", true).await;
    let never = source(&pool, "Never", "never.example.com", true).await;
    let new = source(&pool, "New", "new.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", false).await;
    for (id, set) in [
        (
            fresh,
            "last_crawl_run_at = NOW() - INTERVAL '30 minutes', \
             crawl_rate_limited_until = NOW() + INTERVAL '1 hour'",
        ),
        (
            idle,
            "last_crawl_run_at = NOW() - INTERVAL '5 days', created = NOW() - INTERVAL '30 days', \
             last_crawl_outcome = 'FAILED', last_crawl_error_kind = 'Timeout', \
             next_scrape_at = NOW() + INTERVAL '1 hour'",
        ),
        (never, "created = NOW() - INTERVAL '10 days'"),
        (
            new,
            "last_crawl_run_at = date_trunc('hour', NOW(), 'UTC') - INTERVAL '290 minutes'",
        ),
        (
            disabled,
            "created = NOW() - INTERVAL '10 days', \
             crawl_rate_limited_until = NOW() + INTERVAL '1 hour', \
             next_scrape_at = NOW() + INTERVAL '1 hour'",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE listing_sources SET {set} WHERE listing_source_id = $1"
        )))
        .bind(id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }
    for (id, path, scraped, error_kind, created) in [
        (fresh, "/fresh-1", Some("10 minutes"), None, "2 hours"),
        (
            fresh,
            "/fresh-2",
            Some("3 hours"),
            Some("MassRemovalHeld"),
            "2 days",
        ),
        (fresh, "/fresh-3", None, None, "1 hour"),
        (fresh, "/fresh-4", Some("2 days"), None, "5 days"),
        (idle, "/idle-1", Some("4 days"), None, "20 days"),
        (disabled, "/disabled-1", Some("6 days"), None, "20 days"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO listing_source_urls
             (listing_source_id, url, url_class, raw_source_record_key, last_scraped,
              last_error_kind, created)
             VALUES ($1, $2, 'product', $2, {scraped}, $3, NOW() - INTERVAL '{created}')",
            scraped = scraped.map_or("NULL".to_owned(), |age| format!("NOW() - INTERVAL '{age}'")),
        )))
        .bind(id.as_uuid())
        .bind(format!("https://shop.example.com{path}"))
        .bind(error_kind)
        .execute(&pool)
        .await
        .unwrap();
    }

    let overview = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap();
    let stale = list_sources(
        &pool,
        ListingSourceQuery {
            status: Some(SourceStatus::Stale),
            sort: SourceSort::IdleSince,
            direction: SortDirection::Ascending,
            ..ListingSourceQuery::default()
        },
    )
    .await;

    assert_eq!(
        overview.sources.stale, 2,
        "idle and never; not new or disabled"
    );
    assert_eq!(stale.total, overview.sources.stale);
    assert_eq!(
        source_ids(&stale),
        vec![never, idle],
        "longest idle first: never worked since its creation 10 days ago, then 4 days"
    );
    assert_eq!(
        overview
            .freshness
            .longest_idle
            .iter()
            .map(|source| (
                source.listing_source_id,
                source.listing_source_name.as_deref(),
                source.idle_since
            ))
            .collect::<Vec<_>>(),
        vec![
            (never, Some("Never"), None),
            (idle, Some("Idle"), stale.items[1].idle_since)
        ],
        "the stale list's first sources in its order"
    );
    assert!(stale.items[1].idle_since.is_some());
    let scrape_ages = overview
        .freshness
        .since_last_scrape
        .expect("two scraped sources");
    assert_eq!(scrape_ages.sources, 2, "the disabled source is left out");
    assert!(
        (4 * 86_400 - 60..=4 * 86_400 + 60).contains(&scrape_ages.max_secs),
        "the idle source's 4 days: {scrape_ages:?}"
    );
    assert_eq!(
        overview.freshness.since_last_crawl.map(|ages| ages.sources),
        Some(3)
    );
    assert_eq!(overview.throughput.pages_scraped_1h, 1);
    assert_eq!(
        overview.throughput.pages_scraped_24h, 2,
        "the page whose removal was held 3 hours ago too"
    );
    assert_eq!(overview.throughput.product_urls_found_24h, 2);
    assert_eq!(overview.throughput.crawls_finished_1h, 1);
    assert_eq!(overview.throughput.crawls_finished_24h, 2);
    let hours = overview.throughput.crawl_runs_by_hour;
    let now = OffsetDateTime::now_utc();
    assert!(
        hours[23].hour <= now && now - hours[23].hour < time::Duration::hours(1),
        "the running hour last: {hours:?}"
    );
    assert!(
        hours
            .windows(2)
            .all(|pair| pair[1].hour - pair[0].hour == time::Duration::hours(1)),
        "one entry per clock hour: {hours:?}"
    );
    assert_eq!(
        hours[18].sources, 1,
        "the run 4 hours 50 minutes before the running hour began: {hours:?}"
    );
    assert_eq!(
        hours.iter().map(|hour| hour.sources).sum::<i64>(),
        2,
        "the run of 30 minutes ago too, not the one of 5 days ago: {hours:?}"
    );
    assert_eq!(
        serde_json::to_value(overview.pages).unwrap(),
        json!({
            "total": 5, "scraped_within_a_day": 2, "due": 1, "overdue": 1, "never_scraped": 1
        }),
        "the pages of the crawl-enabled sources by their last scrape"
    );
    assert_eq!(
        (
            overview.sources.rate_limited,
            overview.sources.scrape_paused
        ),
        (1, 1),
        "the disabled source is left out"
    );
    for (status, counted, listed) in [
        (
            SourceStatus::RateLimited,
            overview.sources.rate_limited,
            fresh,
        ),
        (
            SourceStatus::ScrapePaused,
            overview.sources.scrape_paused,
            idle,
        ),
    ] {
        let list = list_sources(
            &pool,
            ListingSourceQuery {
                status: Some(status),
                ..ListingSourceQuery::default()
            },
        )
        .await;
        assert_eq!(list.total, counted, "{status:?}");
        assert_eq!(source_ids(&list), vec![listed], "{status:?}");
    }
    assert_eq!(
        serde_json::to_value(overview.crawls).unwrap(),
        json!({
            "total": 4, "crawled_within_a_day": 2, "due": 0, "overdue": 1, "never_crawled": 1
        }),
        "the crawl-enabled sources by their last spider run"
    );
    assert_eq!(overview.sources.crawl_failing, 1);
    assert_eq!(
        serde_json::to_value(&overview.failures.crawl).unwrap(),
        json!([{ "error_kind": "Timeout", "sources": 1 }])
    );
}

/// The reviews opened, approved and rejected in each of the last seven 24-hour windows.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_report_the_review_flow_of_each_of_the_last_days() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let shop = source(&pool, "Shop", "shop.example.com", true).await;
    let pending = pending_schema_review(&repository, shop).await;
    let approved = closed_schema_review(&repository, shop, "APPROVED").await;
    let rejected = closed_schema_review(&repository, shop, "REJECTED").await;
    let superseded = closed_schema_review(&repository, shop, "SUPERSEDED").await;
    let long_ago = closed_schema_review(&repository, shop, "APPROVED").await;
    for (review_id, created, reviewed) in [
        (pending, "30 hours", None),
        (approved, "50 hours", Some("26 hours")),
        (rejected, "2 hours", Some("1 hour")),
        (superseded, "3 hours", Some("2 hours")),
        (long_ago, "9 days", Some("8 days")),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE crawler_reviews
             SET created = NOW() - INTERVAL '{created}', reviewed = {reviewed}
             WHERE review_id = $1",
            reviewed =
                reviewed.map_or("NULL".to_owned(), |age| format!("NOW() - INTERVAL '{age}'")),
        )))
        .bind(review_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }

    let flow = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap()
        .review_flow;

    assert_eq!(
        serde_json::to_value(flow.daily).unwrap(),
        json!([
            { "opened": 0, "approved": 0, "rejected": 0 },
            { "opened": 0, "approved": 0, "rejected": 0 },
            { "opened": 0, "approved": 0, "rejected": 0 },
            { "opened": 0, "approved": 0, "rejected": 0 },
            { "opened": 1, "approved": 0, "rejected": 0 },
            { "opened": 1, "approved": 1, "rejected": 0 },
            { "opened": 2, "approved": 0, "rejected": 1 }
        ]),
        "oldest window first; the superseded review counts as opened only"
    );
    assert_eq!(
        flow.median_decision_secs_7d,
        Some(45_000),
        "between the approval after 24 hours and the rejection after 1 hour"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_what_the_crawler_did_to_listing_sources_newest_first() {
    let pool = get_postgres_client().await;
    let failing = source(&pool, "Failing", "failing.example.com", true).await;
    let crawled = source(&pool, "Crawled", "crawled.example.com", true).await;
    let old = source(&pool, "Old", "old.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", false).await;
    for (id, set) in [
        (
            failing,
            "scrape_failure_streak = 14, last_scrape_error_kind = 'TIMEOUT', scrape_failing_since = NOW() - INTERVAL '1 hour'",
        ),
        (
            crawled,
            "last_crawl_outcome = 'COMPLETED', last_crawl_run_at = NOW() - INTERVAL '2 hours'",
        ),
        (
            old,
            "last_crawl_outcome = 'COMPLETED', last_crawl_run_at = NOW() - INTERVAL '3 days'",
        ),
        (
            disabled,
            "last_crawl_outcome = 'FAILED', last_crawl_run_at = NOW() - INTERVAL '1 hour'",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE listing_sources SET {set} WHERE listing_source_id = $1"
        )))
        .bind(id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }
    let reader = SourceEventReader::new(pool.clone());
    let since = OffsetDateTime::now_utc() - time::Duration::days(1);

    let (events, truncated) = reader.list_source_events(since, 10).await.unwrap();
    let (first, more) = reader.list_source_events(since, 1).await.unwrap();

    assert_eq!(
        events
            .iter()
            .map(|event| (event.kind, event.listing_source_id))
            .collect::<Vec<_>>(),
        vec![
            (SourceEventKind::ScrapeFailing, failing),
            (SourceEventKind::CrawlFinished, crawled)
        ],
        "newest first; a crawl older than the window and a disabled source are not listed"
    );
    assert!(!truncated);
    assert_eq!(events[0].count, 14);
    assert_eq!(events[0].error_kind.as_deref(), Some("TIMEOUT"));
    assert_eq!(events[1].outcome.as_deref(), Some("COMPLETED"));
    assert_eq!((first.len(), more), (1, true));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_review_events_newest_first_when_asked() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 5);
    let shop = source(&pool, "Shop", "shop.example.com", true).await;
    let review = pending_schema_review(&repository, shop).await;
    for note in ["first", "second", "third"] {
        repository
            .request_repair(review, ReviewActor::AgentReviewer, Some(note))
            .await
            .unwrap();
    }
    let since = OffsetDateTime::now_utc() - time::Duration::hours(1);

    let (oldest_first, _) = repository
        .list_review_events_since(since, 10, SortDirection::Ascending)
        .await
        .unwrap();
    let (newest_first, truncated) = repository
        .list_review_events_since(since, 2, SortDirection::Descending)
        .await
        .unwrap();

    let notes = |events: &[crawler::review::model::ReviewEventSummary]| {
        events
            .iter()
            .map(|event| event.notes.clone().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    assert_eq!(notes(&oldest_first), ["first", "second", "third"]);
    assert_eq!(notes(&newest_first), ["third", "second"]);
    assert!(truncated);
}

async fn insert_product_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    path: &str,
    failure: (&str, Option<i32>, i32, Option<&str>),
) {
    let (kind, status_code, failures, message) = failure;
    sqlx::query(
        "INSERT INTO listing_source_urls
         (listing_source_id, url, url_class, raw_source_record_key, failure_count,
          last_error_kind, last_status_code, last_error_message)
         VALUES ($1, $2, 'product', $2, $3, $4, $5, $6)",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("https://shop.example.com{path}"))
    .bind(failures)
    .bind(kind)
    .bind(status_code)
    .bind(message)
    .execute(pool)
    .await
    .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_count_a_failed_crawl_as_needing_attention_and_say_why_a_source_fails() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool, 1);
    let crawl_failing = source(&pool, "Crawl failing", "crawl-failing.example.com", true).await;
    let scrape_failing = source(&pool, "Scrape failing", "scrape-failing.example.com", true).await;
    let healthy = source(&pool, "Healthy", "healthy.example.com", true).await;
    let disabled = source(&pool, "Disabled", "disabled.example.com", false).await;
    for (id, set) in [
        (
            crawl_failing,
            "last_crawl_outcome = 'FAILED', last_crawl_error_kind = 'CloudflareChallenge', crawl_failure_count = 1",
        ),
        (
            scrape_failing,
            "scrape_failure_streak = 4, last_scrape_error_kind = 'HTTP_STATUS_BURST', last_scrape_status_code = 403",
        ),
        (disabled, "last_crawl_outcome = 'FAILED'"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE listing_sources SET {set} WHERE listing_source_id = $1"
        )))
        .bind(id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }
    let long_message = "x".repeat(500);
    for (path, failure) in [
        (
            "/a",
            ("HttpError", Some(403), 5, Some(long_message.as_str())),
        ),
        ("/b", ("Timeout", None, 2, None)),
        ("/c", ("HttpError", Some(500), 1, Some("boom"))),
        ("/d", ("HttpError", Some(502), 1, Some("bad gateway"))),
        ("/waits", ("PendingSchemaReview", None, 9, None)),
        ("/held", ("MassRemovalHeld", None, 9, None)),
        ("/missing", ("MissingRequiredValue", None, 9, None)),
    ] {
        insert_product_url(&pool, scrape_failing, path, failure).await;
    }
    let overviews = ListingSourceOverviewReader::new(pool.clone());

    let counts = overview_reader(&pool, &repository)
        .overview()
        .await
        .unwrap();
    let failing = list_sources(
        &pool,
        ListingSourceQuery {
            status: Some(SourceStatus::CrawlFailing),
            ..ListingSourceQuery::default()
        },
    )
    .await;
    let attention = list_sources(
        &pool,
        ListingSourceQuery {
            status: Some(SourceStatus::NeedsAttention),
            ..ListingSourceQuery::default()
        },
    )
    .await;
    let healthy_sources = list_sources(
        &pool,
        ListingSourceQuery {
            status: Some(SourceStatus::Healthy),
            ..ListingSourceQuery::default()
        },
    )
    .await;
    let detail = ListingSourceDetailReader::new(overviews.clone())
        .find_listing_source(scrape_failing)
        .await
        .unwrap()
        .expect("the source exists");

    assert_eq!(
        counts.sources.crawl_failing, 1,
        "a disabled source is not counted"
    );
    assert_eq!(counts.sources.needs_attention, 2);
    assert_eq!(source_ids(&failing), vec![crawl_failing]);
    assert_eq!(
        sorted(&source_ids(&attention)),
        sorted(&[crawl_failing, scrape_failing])
    );
    assert_eq!(source_ids(&healthy_sources), vec![healthy]);
    assert_eq!(
        attention.items[0]
            .crawl_state
            .last_crawl_error_kind
            .as_deref(),
        Some("CloudflareChallenge"),
        "a failed crawl ranks ahead of a scrape failure"
    );
    let scrape_state = &detail.overview.crawl_state;
    assert_eq!(scrape_state.last_scrape_status_code, Some(403));
    let urls = detail
        .failing_urls
        .iter()
        .map(|url| (url.url.as_str(), url.error_kind.as_str(), url.status_code))
        .collect::<Vec<_>>();
    assert_eq!(
        urls,
        vec![
            ("https://shop.example.com/a", "HttpError", Some(403)),
            ("https://shop.example.com/b", "Timeout", None),
            ("https://shop.example.com/d", "HttpError", Some(502)),
        ],
        "the most repeated failures first, at most three, without the waits and holds"
    );
    assert_eq!(
        detail.failing_urls[0]
            .error_message
            .as_ref()
            .map(String::len),
        Some(200)
    );
    let healthy_detail = ListingSourceDetailReader::new(overviews)
        .find_listing_source(healthy)
        .await
        .unwrap()
        .expect("the source exists");
    assert!(healthy_detail.failing_urls.is_empty());
}

/// The `label` filter lists the sources that carry that label, the retired one included.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_filter_listing_sources_by_their_label() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let closed = source(&pool, "Closed", "closed.example.com", true).await;
    let gallery = source(&pool, "Gallery", "gallery.example.com", true).await;
    let retired = source(&pool, "Retired", "retired.example.com", false).await;
    source(&pool, "Unlabelled", "plain.example.com", true).await;
    for (listing_source_id, label) in [
        (closed, SourceLabel::ShopClosed),
        (gallery, SourceLabel::NotAnEshop),
    ] {
        repository
            .label_source(
                &listing_source_id,
                ReviewActor::Human,
                &SourceLabelRequest::new(label, None).unwrap(),
            )
            .await
            .unwrap();
    }
    sqlx::query(
        "UPDATE listing_sources SET source_label = 'APPROACH_NOT_APPLICABLE', \
         source_labelled_at = NOW(), source_labelled_by = 'AGENT_AUTHOR' \
         WHERE listing_source_id = $1",
    )
    .bind(retired.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let with = |label| ListingSourceQuery {
        label: Some(label),
        ..ListingSourceQuery::default()
    };

    assert_eq!(
        source_ids(&list_sources(&pool, with(SourceLabel::ShopClosed)).await),
        vec![closed]
    );
    assert_eq!(
        source_ids(&list_sources(&pool, with(SourceLabel::ApproachNotApplicable)).await),
        vec![retired]
    );
    assert!(source_ids(&list_sources(&pool, with(SourceLabel::Other)).await).is_empty());
}

/// A source's held removals list the URLs its overview counts as held, the most recently held
/// first, in windows, and leave out other failures, other sources and inactive URLs.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_the_held_removals_of_a_listing_source() {
    let pool = get_postgres_client().await;
    let shop = source(&pool, "Shop", "shop.example.com", true).await;
    let other = source(&pool, "Other", "other.example.com", true).await;
    for (id, path, kind, held_ago, disposition, title) in [
        (
            shop,
            "/older",
            "MassRemovalHeld",
            "3 hours",
            "ACTIVE",
            Some("Oak chair"),
        ),
        (
            shop,
            "/newer",
            "MassRemovalHeld",
            "1 hour",
            "DORMANT_SOLD",
            None,
        ),
        (shop, "/failing", "Timeout", "1 hour", "ACTIVE", None),
        (
            shop,
            "/duplicate",
            "MassRemovalHeld",
            "1 hour",
            "DUPLICATE",
            None,
        ),
        (
            other,
            "/elsewhere",
            "MassRemovalHeld",
            "1 hour",
            "ACTIVE",
            None,
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO listing_source_urls
             (listing_source_id, url, url_class, raw_source_record_key, crawler_disposition,
              last_error_kind, last_error_message, last_status_code, last_scraped,
              last_captured_title)
             VALUES ($1, $2, 'product', $2, $3, $4, 'held: 12 of the 12 pages would go', 404,
                     NOW() - INTERVAL '{held_ago}', $5)"
        )))
        .bind(id.as_uuid())
        .bind(format!("https://shop.example.com{path}"))
        .bind(disposition)
        .bind(kind)
        .bind(title)
        .execute(&pool)
        .await
        .unwrap();
    }
    let reader = HeldRemovalReader::new(pool.clone());

    let all = reader.list(shop, ListWindow::default()).await.unwrap();
    let second = reader.list(shop, window(1, 1)).await.unwrap();
    let overview = ListingSourceOverviewReader::new(pool.clone())
        .find_listing_source(shop)
        .await
        .unwrap()
        .expect("the source exists");

    assert_eq!(all.total, overview.held_removal_urls);
    assert_eq!(
        all.items
            .iter()
            .map(|url| url.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://shop.example.com/newer",
            "https://shop.example.com/older"
        ]
    );
    assert_eq!(all.items[1].title.as_deref(), Some("Oak chair"));
    assert_eq!(all.items[1].status_code, Some(404));
    assert!(
        all.items[1]
            .message
            .as_deref()
            .is_some_and(|message| message.contains("12 of the 12")),
        "{:?}",
        all.items[1].message
    );
    assert_eq!((second.total, second.items.len()), (2, 1));
    assert_eq!(second.items[0].url, "https://shop.example.com/older");
}
