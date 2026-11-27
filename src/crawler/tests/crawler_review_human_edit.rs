//! A human's edit of a ListingSource's live URL pattern or product schemas through a review: the
//! review opens with the live configuration as its candidate, a second request answers the pending
//! review, the review stays with humans in `agent` mode, and approving the unchanged candidate
//! keeps the live configuration.

use crawler::CrawlerReviewId;
use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::model::CrawlerReviewEvent;
use crawler::review::model::{
    ArtifactType, PAGE_ROLE_SEED, PageSize, ReviewActor, ReviewEventAction, ReviewWaiting,
    STATUS_APPROVED, UrlPatternDecision, UrlPatternReviewCandidate,
};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::readers::{
    ConsoleOverviewReader, ListingSourceOverviewReader, ReviewQuery, ReviewQueueReader,
};
use crawler::review::repository::{
    Approval, CrawlerReviewRepository, HUMAN_EDIT_REASON, PendingReviewQuery, ReviewPlacement,
    ReviewRepositoryError,
};
use crawler::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crawler::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use crawler::scraper::css_selector::removed_page_schema_repository::{
    RemovedPageSchemaRepository, RemovedPageSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::classification::product_url_pattern::UrlPatternMatching;
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

const SHOP: &str = "https://shop.example";

fn rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

fn live_schema() -> ProductCssSelectorSchema {
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

fn live_template() -> RemovedPageSchema {
    RemovedPageSchema {
        selector: "p.notice".into(),
        text: Some("This item has been sold".to_owned()),
        regex: None,
    }
}

/// The product page the shop serves for the item `slug`: one sold, every other in stock.
fn shop_page(slug: &str) -> String {
    let state = if slug == "sold-table" {
        "Sold"
    } else {
        "In stock"
    };
    format!(
        "<html><body><span class=\"id\">{slug}</span><h1>{slug}</h1><span class=\"state\">{state}</span><img class=\"product\" src=\"{slug}.jpg\"></body></html>"
    )
}

fn agent_mode() -> AgentDecisionPolicy {
    AgentDecisionPolicy::new(CrawlerAuthoringMode::Agent, AgentLimits::default())
}

/// A crawl-enabled ListingSource with a crawl root on the shop and the live URL pattern
/// `/item/<slug>` without `gift-card`, matched against the path and query.
async fn listing_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, url_pattern, url_pattern_exclude, url_pattern_state, \
          url_pattern_matching, created, updated) \
         VALUES ($1, 'Edited shop', 'edited-shop', TRUE, $2, 'shop.example', \
                 '/item/[a-z-]+', 'gift-card', 'MATCHED', 'PATH_AND_QUERY_FULL_MATCH', \
                 NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("{SHOP}/"))
    .execute(pool)
    .await
    .unwrap();
    listing_source_id
}

/// Stores the source's live schema and removed-page template and its product URLs: `captured`,
/// whose listings were captured, scraped one after the other, and a product URL never captured.
async fn store_live_schemas_and_urls(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    captured: &[&str],
) {
    let now = OffsetDateTime::now_utc();
    ListingSourceProductSchemaRepositoryImpl::new(pool)
        .insert_product_schema(
            &listing_source_id,
            &ListingSourceProductSchema {
                listing_source_id,
                product_schemas: vec![live_schema()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
    RemovedPageSchemaRepositoryImpl::new(pool)
        .insert_removed_page_schema(
            &listing_source_id,
            &ListingSourceRemovedPageSchema {
                listing_source_id,
                removed_page_schemas: vec![live_template()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
    for (age, slug) in captured.iter().enumerate() {
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, url, url_class, raw_source_record_key, last_source_listing_id, \
              last_scraped) \
             VALUES ($1, $2, 'product', $2, $3, NOW() - make_interval(mins => $4))",
        )
        .bind(listing_source_id.as_uuid())
        .bind(format!("{SHOP}/item/{slug}"))
        .bind(*slug)
        .bind(i32::try_from(age).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    store_url(pool, listing_source_id, "/item/never-captured", "product").await;
}

async fn store_url(pool: &PgPool, listing_source_id: ListingSourceId, path: &str, class: &str) {
    sqlx::query(
        "INSERT INTO listing_source_urls (listing_source_id, url, url_class, raw_source_record_key) \
         VALUES ($1, $2, $3, $2)",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("{SHOP}{path}"))
    .bind(class)
    .execute(pool)
    .await
    .unwrap();
}

async fn open(
    repository: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
    artifact: ArtifactType,
) -> Result<ReviewPlacement, ReviewRepositoryError> {
    repository
        .open_human_edit_review(&listing_source_id, artifact, ReviewActor::Human)
        .await
}

fn opened(placement: ReviewPlacement) -> CrawlerReviewId {
    match placement {
        ReviewPlacement::Opened(review_id) => review_id,
        ReviewPlacement::Joined(review_id) => panic!("review {review_id} was pending already"),
    }
}

async fn approve(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Result<(), ReviewRepositoryError> {
    let expected_candidate_version = repository.candidate_version(review_id).await?;
    repository
        .approve_review(
            review_id,
            ReviewActor::Human,
            Approval {
                expected_candidate_version,
                notes: None,
                incomplete_coverage: IncompleteCoverage::Refuse,
            },
        )
        .await
}

/// Evaluates the review's pages, each with the shop's page, and stores the matrix.
async fn store_live_matrix(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) {
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| {
            let slug = page.url.rsplit('/').next().unwrap_or_default().to_owned();
            (page, shop_page(&slug))
        })
        .collect();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages,
            Vec::new(),
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
        .await
        .unwrap();
}

fn hand_overs(events: &[CrawlerReviewEvent]) -> Vec<&CrawlerReviewEvent> {
    events
        .iter()
        .filter(|event| event.action == ReviewEventAction::HandedToHuman)
        .collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_product_schema_review_of_the_live_schemas_over_recently_captured_pages() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = listing_source(&pool).await;
    store_live_schemas_and_urls(
        &pool,
        listing_source_id,
        &["chair", "sold-table", "lamp", "clock", "desk", "mirror"],
    )
    .await;

    let review_id = opened(
        open(&repository, listing_source_id, ArtifactType::ProductSchema)
            .await
            .unwrap(),
    );

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        (
            detail.review.artifact_type.as_str(),
            detail.review.status.as_str(),
            detail.review.reason.as_str()
        ),
        ("PRODUCT_SCHEMA", "PENDING_REVIEW", HUMAN_EDIT_REASON)
    );
    assert_eq!(
        detail.review.candidate_payload,
        json!({ "schemas": [live_schema()], "removed_page_schemas": [live_template()] })
    );
    let mut pages: Vec<(String, &str)> = detail
        .pages
        .iter()
        .map(|page| (page.url.clone(), page.role.as_str()))
        .collect();
    pages.sort();
    let mut recent: Vec<(String, &str)> = ["chair", "sold-table", "lamp", "clock", "desk"]
        .map(|slug| (format!("{SHOP}/item/{slug}"), PAGE_ROLE_SEED))
        .to_vec();
    recent.sort();
    assert_eq!(
        pages, recent,
        "the five most recently scraped captured product pages"
    );
    let hand_overs = hand_overs(&detail.events);
    assert_eq!(hand_overs.len(), 1, "{:?}", detail.events);
    assert_eq!(hand_overs[0].actor, ReviewActor::System);
    assert_eq!(hand_overs[0].details["reason"], "HUMAN_EDIT");
    assert!(
        hand_overs[0]
            .notes
            .as_deref()
            .is_some_and(|note| note.contains("live product schemas")),
        "{:?}",
        hand_overs[0].notes
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_answer_the_pending_review_of_the_artifact_instead_of_opening_another() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = listing_source(&pool).await;
    store_live_schemas_and_urls(&pool, listing_source_id, &["chair"]).await;

    let schemas = opened(
        open(&repository, listing_source_id, ArtifactType::ProductSchema)
            .await
            .unwrap(),
    );
    let pattern = opened(
        open(&repository, listing_source_id, ArtifactType::UrlPattern)
            .await
            .unwrap(),
    );

    assert_eq!(
        open(&repository, listing_source_id, ArtifactType::ProductSchema)
            .await
            .unwrap(),
        ReviewPlacement::Joined(schemas)
    );
    assert_eq!(
        open(&repository, listing_source_id, ArtifactType::UrlPattern)
            .await
            .unwrap(),
        ReviewPlacement::Joined(pattern)
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crawler_reviews \
         WHERE listing_source_id = $1 AND status = 'PENDING_REVIEW'",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 2);
}

/// The URL-pattern review holds the live decision, its exclusion and matching, as both the current
/// pattern and the candidate, over the source's stored URLs classified under it; approving it
/// unchanged keeps the live decision.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_url_pattern_review_of_the_live_decision_that_approves_unchanged() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = listing_source(&pool).await;
    store_url(&pool, listing_source_id, "/item/chair", "product").await;
    store_url(&pool, listing_source_id, "/item/gift-card", "other").await;
    store_url(&pool, listing_source_id, "/about", "other").await;

    let review_id = opened(
        open(&repository, listing_source_id, ArtifactType::UrlPattern)
            .await
            .unwrap(),
    );

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.reason, HUMAN_EDIT_REASON);
    let candidate: UrlPatternReviewCandidate =
        serde_json::from_value(detail.review.candidate_payload.clone()).unwrap();
    assert_eq!(
        candidate.decision,
        UrlPatternDecision::Pattern {
            value: "/item/[a-z-]+".to_owned(),
            exclude: Some("gift-card".to_owned()),
            matching: UrlPatternMatching::PathAndQueryFullMatch,
        }
    );
    assert_eq!(
        (
            candidate.current_pattern.as_deref(),
            candidate.current_exclude.as_deref()
        ),
        (Some("/item/[a-z-]+"), Some("gift-card"))
    );
    let mut sample: Vec<String> = detail
        .urls
        .iter()
        .map(|url| {
            format!(
                "{} {} {:?} {:?}",
                url.url.trim_start_matches(SHOP),
                url.previous_class.as_deref().unwrap_or("-"),
                url.current_pattern_match,
                url.candidate_pattern_match,
            )
        })
        .collect();
    sample.sort();
    assert_eq!(
        sample,
        [
            "/about other Some(false) Some(false)",
            "/item/chair product Some(true) Some(true)",
            "/item/gift-card other Some(false) Some(false)",
        ]
    );
    assert_eq!(detail.review.validation_summary["sample_count"], 3);
    assert_eq!(
        detail.review.validation_summary["candidate_product_count"],
        1
    );

    approve(&repository, review_id).await.unwrap();

    let approved = repository.get_review(review_id).await.unwrap();
    assert_eq!(approved.review.status, STATUS_APPROVED);
    let live: (String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern_state, url_pattern, url_pattern_exclude, url_pattern_matching \
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        live,
        (
            "MATCHED".to_owned(),
            Some("/item/[a-z-]+".to_owned()),
            Some("gift-card".to_owned()),
            "PATH_AND_QUERY_FULL_MATCH".to_owned()
        )
    );
}

/// A source whose decision is `NO_PATTERN` opens with that decision, one never decided with an
/// undecided candidate the human writes the pattern for.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_url_pattern_review_of_a_no_pattern_or_undecided_source() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    for (state, expected) in [
        ("NO_PATTERN", UrlPatternDecision::NoPattern),
        ("UNKNOWN", UrlPatternDecision::Undecided),
    ] {
        let listing_source_id = listing_source(&pool).await;
        sqlx::query(
            "UPDATE listing_sources SET url_pattern = NULL, url_pattern_exclude = NULL, \
             url_pattern_matching = NULL, url_pattern_state = $2 WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .bind(state)
        .execute(&pool)
        .await
        .unwrap();

        let review_id = opened(
            open(&repository, listing_source_id, ArtifactType::UrlPattern)
                .await
                .unwrap(),
        );

        let candidate: UrlPatternReviewCandidate = serde_json::from_value(
            repository
                .get_review(review_id)
                .await
                .unwrap()
                .review
                .candidate_payload,
        )
        .unwrap();
        assert_eq!(candidate.decision, expected, "{state}");
        assert_eq!(candidate.current_pattern, None, "{state}");
        // One crawl-enabled source per crawl host, so the next one takes the shop's.
        sqlx::query(
            "UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    }
}

/// Approving the unchanged live schemas once the live matrix covers the pages is allowed and
/// keeps the live schemas and templates.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_approve_the_unchanged_live_schemas_once_the_matrix_covers_the_pages() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = listing_source(&pool).await;
    store_live_schemas_and_urls(&pool, listing_source_id, &["chair", "sold-table"]).await;
    let review_id = opened(
        open(&repository, listing_source_id, ArtifactType::ProductSchema)
            .await
            .unwrap(),
    );
    store_live_matrix(&repository, review_id).await;

    approve(&repository, review_id).await.unwrap();

    assert_eq!(
        repository
            .get_review(review_id)
            .await
            .unwrap()
            .review
            .status,
        STATUS_APPROVED
    );
    let live = ListingSourceProductSchemaRepositoryImpl::new(&pool)
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|live| live.product_schemas);
    assert_eq!(live, Some(vec![live_schema()]));
    let templates = RemovedPageSchemaRepositoryImpl::new(&pool)
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|stored| stored.removed_page_schemas);
    assert_eq!(templates, Some(vec![live_template()]));
}

/// In `agent` mode the review waits for a human from the moment it opens, stays so when the human
/// edits it, never enters the agents' queue or counts as a review the agents left to a human, gets
/// no final round, and refuses the agents' changes.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_a_review_a_human_opened_to_edit_with_humans_in_agent_mode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(agent_mode());
    let listing_source_id = listing_source(&pool).await;
    let review_id = opened(
        open(&repository, listing_source_id, ArtifactType::UrlPattern)
            .await
            .unwrap(),
    );
    let human_required = || async {
        repository
            .get_review(review_id)
            .await
            .unwrap()
            .review
            .human_required
    };
    assert!(human_required().await);

    let candidate = repository
        .get_review(review_id)
        .await
        .unwrap()
        .review
        .candidate_payload;
    let mut edited = candidate.clone();
    edited["decision"]["value"] = json!("/item/[a-z0-9-]+");
    let version = repository.candidate_version(review_id).await.unwrap();
    repository
        .update_candidate_payload(review_id, ReviewActor::Human, version, edited.clone())
        .await
        .unwrap();
    assert!(
        human_required().await,
        "the human's own edit leaves the review with humans"
    );

    let version = repository.candidate_version(review_id).await.unwrap();
    assert!(matches!(
        repository
            .update_candidate_payload(review_id, ReviewActor::AgentAuthor, version, edited)
            .await,
        Err(ReviewRepositoryError::HumanRequired { .. })
    ));
    assert!(matches!(
        repository
            .start_final_round(review_id, ReviewActor::AgentAuthor)
            .await,
        Err(ReviewRepositoryError::FinalRoundNotDue(_))
    ));
    let queue = repository
        .list_pending_reviews(PendingReviewQuery {
            human_required: None,
            after: None,
            page_size: PageSize::MAX,
        })
        .await
        .unwrap();
    assert!(
        queue
            .reviews
            .iter()
            .all(|review| review.review_id != review_id),
        "the agents' queue leaves the review out"
    );
    assert_eq!(
        (queue.totals.workable, queue.totals.waiting_for_human),
        (0, 0)
    );
    let overview =
        ConsoleOverviewReader::new(&repository, ListingSourceOverviewReader::new(pool.clone()))
            .overview()
            .await
            .unwrap();
    assert_eq!(overview.reviews.waiting_for_human, 0);
    let waiting = ReviewQueueReader::of(&repository)
        .list_reviews(&ReviewQuery {
            waiting: Some(ReviewWaiting::Human),
            ..ReviewQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(waiting.total, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_an_edit_the_source_cannot_have_reviewed_or_an_agent_asks_for() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = listing_source(&pool).await;

    assert!(matches!(
        open(&repository, listing_source_id, ArtifactType::ProductSchema).await,
        Err(ReviewRepositoryError::HumanEditNotPossible(..))
    ));
    sqlx::query(
        "UPDATE listing_sources SET crawl_root_url = NULL, crawl_host = NULL \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        open(&repository, listing_source_id, ArtifactType::UrlPattern).await,
        Err(ReviewRepositoryError::HumanEditNotPossible(..))
    ));
    for agent in [ReviewActor::AgentAuthor, ReviewActor::AgentReviewer] {
        assert!(matches!(
            repository
                .open_human_edit_review(&listing_source_id, ArtifactType::UrlPattern, agent)
                .await,
            Err(ReviewRepositoryError::HumanEditNeedsHuman)
        ));
    }
    assert!(matches!(
        open(
            &repository,
            ListingSourceId::new(),
            ArtifactType::UrlPattern
        )
        .await,
        Err(ReviewRepositoryError::ListingSourceNotFound(_))
    ));
    let reviews: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM crawler_reviews WHERE listing_source_id = $1")
            .bind(listing_source_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reviews, 0);
}
