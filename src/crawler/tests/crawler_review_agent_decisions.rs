//! The review agents' decisions and a human's restore: the reviewer agent approves and rejects
//! only while agent decisions are `guarded`, as in `agent` authoring mode, and the author agent
//! never; the reviewer agent changes no candidate; every approval names the candidate version it
//! was based on; an agent approval passes the four-eyes rule and the approval guard on the
//! server's own live evaluation; the agents' decisions of an hour are capped from the event trail;
//! and a human restores what an approval replaced while it is still live.

use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
use crawler::review::approval_guard::{FailedApprovalCheck, GuardedPage};
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, ReviewActor, ReviewEventAction, STATUS_APPROVED, STATUS_PENDING_REVIEW,
    SampleLabelInput, SampleUrlLabel, SchemaReviewPageInput, UrlPatternDecision,
};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::classification::url_classifier::UrlClassifier;
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sqlx::PgPool;
use std::num::NonZeroU32;
use std::time::Duration;
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

fn schema(title_selector: &str) -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: rule(title_selector),
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

fn notice_template(text: &str) -> RemovedPageSchema {
    RemovedPageSchema {
        selector: "p.notice".into(),
        text: Some(text.to_owned()),
        regex: None,
    }
}

/// The product page the shop serves at `url`: a chair titled in an `h1`, or a table titled in an
/// `h2`, so a schema per template covers each.
fn shop_page(url: &str) -> String {
    let (slug, title_tag) = if url.contains("table") {
        ("table", "h2")
    } else {
        ("chair", "h1")
    };
    format!(
        "<html><body><span class=\"id\">{slug}</span><{title_tag}>{slug}</{title_tag}><span class=\"state\">In stock</span><img class=\"product\" src=\"{slug}.jpg\"></body></html>"
    )
}

fn page(slug: &str) -> SchemaReviewPageInput {
    let url = format!("https://example.com/products/{slug}");
    SchemaReviewPageInput {
        raw_html: shop_page(&url),
        url,
        role: PAGE_ROLE_PRIMARY.to_owned(),
    }
}

fn guarded_policy(max_decisions_per_hour: u32) -> AgentDecisionPolicy {
    AgentDecisionPolicy {
        approvals: AgentApprovals::Guarded,
        max_decisions_per_hour: NonZeroU32::new(max_decisions_per_hour).unwrap_or(NonZeroU32::MIN),
        ..AgentDecisionPolicy::default()
    }
}

/// A repository taking the review agent's guarded decisions, at most `max_decisions_per_hour`.
fn guarded(pool: &PgPool, max_decisions_per_hour: u32) -> CrawlerReviewRepository {
    CrawlerReviewRepository::new(pool.clone())
        .with_agent_decisions(guarded_policy(max_decisions_per_hour))
}

/// A crawl-enabled ListingSource on `example.com` and its domain.
async fn listing_source(pool: &PgPool) -> (ListingSourceId, CrawlerDomainId) {
    listing_source_on(pool, "example.com").await
}

/// A crawl-enabled ListingSource on `domain`, which no other ListingSource may hold, and that
/// domain.
async fn listing_source_on(pool: &PgPool, domain: &str) -> (ListingSourceId, CrawlerDomainId) {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Agent source', $2, TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("agent-source-{}", listing_source_id.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, $3, $3)",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .bind(domain)
    .execute(pool)
    .await
    .unwrap();
    (listing_source_id, domain_id)
}

async fn set_live_schemas(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
    schemas: &[ProductCssSelectorSchema],
) {
    sqlx::query(
        "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW()) \
         ON CONFLICT (listing_source_id) DO UPDATE SET product_schema = EXCLUDED.product_schema",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!(schemas))
    .execute(pool)
    .await
    .unwrap();
}

async fn live_schemas(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
) -> Option<Vec<ProductCssSelectorSchema>> {
    ListingSourceProductSchemaRepositoryImpl::new(pool)
        .find_product_schema(listing_source_id)
        .await
        .unwrap()
        .map(|live| live.product_schemas)
}

async fn stored_removed_page_schemas(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT removed_page_schema FROM listing_source_removed_page_schemas \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn pending_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
    reason: &str,
    schemas: &[ProductCssSelectorSchema],
    pages: Vec<SchemaReviewPageInput>,
) -> CrawlerReviewId {
    repository
        .create_schema_review(listing_source_id, reason, schemas, pages, json!({}))
        .await
        .unwrap()
}

async fn version(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) -> i64 {
    repository.candidate_version(review_id).await.unwrap()
}

fn approval(expected_candidate_version: i64) -> Approval<'static> {
    Approval {
        expected_candidate_version,
        notes: Some("checked on the live pages"),
        incomplete_coverage: IncompleteCoverage::Refuse,
    }
}

/// Approves the review's current version as a human who confirmed its coverage, as the console
/// does.
async fn approve_as_human(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) {
    let expected_candidate_version = version(repository, review_id).await;
    let incomplete_coverage = match repository
        .approve_review(
            review_id,
            ReviewActor::Human,
            approval(expected_candidate_version),
        )
        .await
    {
        Ok(()) => return,
        Err(ReviewRepositoryError::IncompleteCoverage { coverage, .. }) => {
            IncompleteCoverage::Acknowledged(coverage)
        }
        Err(error) => panic!("human approval: {error}"),
    };
    repository
        .approve_review(
            review_id,
            ReviewActor::Human,
            Approval {
                incomplete_coverage,
                ..approval(expected_candidate_version)
            },
        )
        .await
        .unwrap();
}

/// Evaluates the review's candidate on its pages as the shop serves them, fetched at
/// `fetched_at`, and stores the matrix as a refresh does.
async fn store_live_matrix(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    fetched_at: OffsetDateTime,
) {
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| {
            let html = shop_page(&page.url);
            (page, html)
        })
        .collect();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, pages, fetched_at)
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, evaluated)
        .await
        .unwrap();
}

async fn trail(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Vec<(ReviewActor, ReviewEventAction)> {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .events
        .iter()
        .map(|event| (event.actor, event.action))
        .collect()
}

fn failed_checks(refusal: Result<(), ReviewRepositoryError>) -> Vec<FailedApprovalCheck> {
    match refusal {
        Err(ReviewRepositoryError::AgentApprovalRefused { failed_checks, .. }) => failed_checks,
        other => panic!("expected the approval guard to refuse: {other:?}"),
    }
}

/// With agent decisions off, as outside `agent` authoring mode, the author agent's staging is kept
/// and every agent decision is left to a human.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_the_agent_verdicts_but_keep_its_staging_while_agent_decisions_are_off() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;

    let edited = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1.agent")] }),
        )
        .await
        .unwrap();
    repository
        .request_repair(
            review_id,
            ReviewActor::AgentAuthor,
            Some("title selector too loose"),
        )
        .await
        .unwrap();
    let approved = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(edited))
        .await;
    let rejected = repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("not a product page"),
        )
        .await;

    assert!(
        matches!(approved, Err(ReviewRepositoryError::AgentDecisionsOff)),
        "{approved:?}"
    );
    assert!(
        matches!(rejected, Err(ReviewRepositoryError::AgentDecisionsOff)),
        "{rejected:?}"
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_PENDING_REVIEW);
    assert_eq!(
        trail(&repository, review_id).await,
        vec![
            (ReviewActor::AgentAuthor, ReviewEventAction::CandidateEdited),
            (ReviewActor::AgentAuthor, ReviewEventAction::RepairRequested),
        ]
    );
}

/// An approval names the version its approver read: an edit made since is never approved
/// unseen, for a human as for the agent.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_an_approval_of_another_candidate_version_from_anyone() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            1,
            0,
            "title",
            Some(rule("h1.edited")),
        )
        .await
        .unwrap();

    for actor in [ReviewActor::Human, ReviewActor::AgentReviewer] {
        let refused = repository
            .approve_review(review_id, actor, approval(1))
            .await;
        assert!(
            matches!(
                refused,
                Err(ReviewRepositoryError::StaleCandidateVersion {
                    expected: 1,
                    current: 2,
                    ..
                })
            ),
            "{actor}: {refused:?}"
        );
    }
    assert_eq!(
        repository
            .get_review(review_id)
            .await
            .unwrap()
            .review
            .status,
        STATUS_PENDING_REVIEW
    );
    assert_eq!(live_schemas(&pool, &listing_source_id).await, None);
}

/// Approval moves the review past the approved version, so an edit someone based on the pending
/// review is refused instead of rewriting the live schemas it never saw become live.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_an_edit_based_on_the_pending_review_once_it_is_approved() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    let read_while_pending = version(&repository, review_id).await;

    approve_as_human(&repository, review_id).await;
    let edit = repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            read_while_pending,
            0,
            "title",
            Some(rule("h1.unseen")),
        )
        .await;

    assert!(
        matches!(
            edit,
            Err(ReviewRepositoryError::StaleCandidateVersion { current, .. })
                if current == read_while_pending + 1
        ),
        "{edit:?}"
    );
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1")])
    );
}

/// The agent approves only on the server's own live evaluation of exactly what goes live, of
/// every current review page, recent enough, with every page covered: never on an
/// acknowledgement, a missing, outdated or expired matrix, or a page no schema covers.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_agent_approval_only_once_a_current_live_matrix_passes_the_guard() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair"), page("table")],
    )
    .await;
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let table = pages
        .iter()
        .find(|page| page.url.ends_with("table"))
        .unwrap();

    let without_matrix = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(1))
        .await;
    let acknowledged = repository
        .approve_review(
            review_id,
            ReviewActor::AgentReviewer,
            Approval {
                incomplete_coverage: IncompleteCoverage::Acknowledged(
                    crawler::review::page_coverage::ReviewPageCoverage {
                        covered: 0,
                        removed: 0,
                        total: 2,
                        not_evaluated: 2,
                    },
                ),
                ..approval(1)
            },
        )
        .await;
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    let uncovered = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(1))
        .await;

    assert_eq!(
        failed_checks(without_matrix),
        vec![FailedApprovalCheck::LiveMatrixMissing]
    );
    assert!(
        matches!(
            acknowledged,
            Err(ReviewRepositoryError::AgentCoverageAcknowledgement)
        ),
        "{acknowledged:?}"
    );
    assert_eq!(
        failed_checks(uncovered),
        vec![FailedApprovalCheck::PagesNotCovered {
            pages: vec![GuardedPage {
                review_page_id: table.review_page_id,
                url: table.url.clone(),
            }],
        }]
    );

    let edited = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1"), schema("h2")] }),
        )
        .await
        .unwrap();
    let dropped = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(edited))
        .await;
    store_live_matrix(
        &repository,
        review_id,
        OffsetDateTime::now_utc() - Duration::from_secs(2 * 60 * 60),
    )
    .await;
    let expired = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(edited))
        .await;

    assert_eq!(
        failed_checks(dropped),
        vec![FailedApprovalCheck::LiveMatrixMissing]
    );
    assert!(
        matches!(
            failed_checks(expired).as_slice(),
            [FailedApprovalCheck::LiveMatrixExpired { .. }]
        ),
        "an evaluation older than the allowed age vouches for nothing"
    );
    assert_eq!(
        trail(&repository, review_id).await,
        vec![(ReviewActor::AgentAuthor, ReviewEventAction::CandidateEdited)]
    );

    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(edited))
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_APPROVED);
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1"), schema("h2")])
    );
    let approved = detail.events.last().unwrap();
    assert_eq!(
        (approved.actor, approved.action),
        (ReviewActor::AgentReviewer, ReviewEventAction::Approved)
    );
    assert_eq!(
        approved.details["live_evaluation"]["candidate_version"],
        edited
    );
    assert_eq!(
        approved.details["coverage"],
        json!({ "covered": 2, "removed": 0, "total": 2, "not_evaluated": 0 })
    );
}

/// A legacy append review goes live merged with the live list, so its live matrix evaluates
/// that merge, and a live list changed since the evaluation outdates it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_evaluate_the_merged_list_an_append_makes_live_and_refuse_it_once_the_live_list_changed()
 {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    set_live_schemas(&pool, &listing_source_id, &[schema("h1")]).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "append_schema_generation",
        &[schema("h2")],
        vec![page("chair"), page("table")],
    )
    .await;

    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    set_live_schemas(&pool, &listing_source_id, &[schema("h1.changed")]).await;
    let outdated = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(1))
        .await;

    assert!(
        matches!(
            failed_checks(outdated).as_slice(),
            [FailedApprovalCheck::LiveMatrixOutdated { .. }]
        ),
        "the matrix evaluated a merge with the live list that changed since"
    );

    set_live_schemas(&pool, &listing_source_id, &[schema("h1")]).await;
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(1))
        .await
        .unwrap();

    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1"), schema("h2")])
    );
}

/// The agent's approvals plus rejections of an hour are capped from the event trail, so a new
/// repository, as after a restart, counts the same decisions; humans keep deciding.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_cap_the_agent_decisions_of_an_hour_from_the_event_trail() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 2);
    let mut reviews = Vec::new();
    for index in 0..4 {
        let (listing_source_id, _) =
            listing_source_on(&pool, &format!("shop-{index}.example")).await;
        reviews.push(
            pending_review(
                &repository,
                &listing_source_id,
                "manual_schema_required",
                &[schema("h1")],
                vec![page("chair")],
            )
            .await,
        );
    }

    for review_id in &reviews[..2] {
        repository
            .reject_review(
                *review_id,
                ReviewActor::AgentReviewer,
                Some("not a shop page"),
            )
            .await
            .unwrap();
    }
    let rejection = repository
        .reject_review(
            reviews[2],
            ReviewActor::AgentReviewer,
            Some("not a shop page"),
        )
        .await;
    let approval_after_restart = guarded(&pool, 2)
        .approve_review(reviews[3], ReviewActor::AgentReviewer, approval(1))
        .await;

    for refused in [rejection, approval_after_restart] {
        assert!(
            matches!(
                refused,
                Err(ReviewRepositoryError::AgentDecisionLimitReached { limit: 2, retry_after })
                    if retry_after > Duration::from_secs(3500)
                        && retry_after <= Duration::from_secs(3600)
            ),
            "{refused:?}"
        );
    }
    assert!(trail(&repository, reviews[2]).await.is_empty());
    repository
        .reject_review(reviews[2], ReviewActor::Human, None)
        .await
        .unwrap();
}

/// Concurrent agent decisions count one after the other, so together they never exceed the
/// cap: each counts the decisions the ones before it committed.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_never_exceed_the_hourly_cap_with_concurrent_agent_decisions() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 2);
    let mut reviews = Vec::new();
    for index in 0..5 {
        let (listing_source_id, _) =
            listing_source_on(&pool, &format!("concurrent-{index}.example")).await;
        reviews.push(
            pending_review(
                &repository,
                &listing_source_id,
                "manual_schema_required",
                &[schema("h1")],
                vec![page("chair")],
            )
            .await,
        );
    }

    let decisions = futures::future::join_all(reviews.iter().map(|review_id| {
        let repository = repository.clone();
        let review_id = *review_id;
        tokio::spawn(async move {
            repository
                .reject_review(
                    review_id,
                    ReviewActor::AgentReviewer,
                    Some("not a shop page"),
                )
                .await
        })
    }))
    .await;

    let (mut taken, mut capped) = (0, 0);
    for decision in decisions {
        match decision.unwrap_or_else(|error| panic!("decision task: {error}")) {
            Ok(()) => taken += 1,
            Err(ReviewRepositoryError::AgentDecisionLimitReached { limit: 2, .. }) => capped += 1,
            Err(error) => panic!("unexpected refusal: {error}"),
        }
    }
    assert_eq!((taken, capped), (2, 3));
    let recorded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crawler_review_events WHERE actor = 'AGENT_REVIEWER' AND action = 'REJECTED'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recorded, 2);
}

/// A human restores the schema list and removed-page templates an agent approval replaced,
/// re-queues the pages waiting on a review as the approval did, and records it; the agent cannot
/// restore, and an approval is restored once.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_restore_what_an_agent_approval_replaced() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, domain_id) = listing_source(&pool).await;
    set_live_schemas(&pool, &listing_source_id, &[schema("h1.old")]).await;
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!([notice_template("sold")]))
    .execute(&pool)
    .await
    .unwrap();
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1.old")],
        vec![page("chair")],
    )
    .await;
    let edited = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1")], "removed_page_schemas": [] }),
        )
        .await
        .unwrap();
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(edited))
        .await
        .unwrap();
    let waiting_url = "https://example.com/products/waiting";
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, url_class, raw_source_record_key, last_error_kind, \
          next_retry_at, schema_review_waited_on, schema_review_wait_streak) \
         VALUES ($1, $2, $3, 'product', $3, 'PendingSchemaReview', NOW() + INTERVAL '1 hour', $4, 1)",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(waiting_url)
    .bind(uuid::Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();

    let by_agents = [
        repository
            .restore_approval(review_id, ReviewActor::AgentReviewer, None)
            .await,
        repository
            .restore_approval(review_id, ReviewActor::AgentAuthor, None)
            .await,
    ];
    repository
        .restore_approval(
            review_id,
            ReviewActor::Human,
            Some("agent schema misses sold pages"),
        )
        .await
        .unwrap();
    let again = repository
        .restore_approval(review_id, ReviewActor::Human, None)
        .await;

    for by_agent in by_agents {
        assert!(
            matches!(by_agent, Err(ReviewRepositoryError::RestoreNeedsHuman)),
            "{by_agent:?}"
        );
    }
    assert!(
        matches!(again, Err(ReviewRepositoryError::ApprovalAlreadyRestored(id)) if id == review_id),
        "{again:?}"
    );
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1.old")])
    );
    assert_eq!(
        stored_removed_page_schemas(&pool, &listing_source_id).await,
        Some(json!([notice_template("sold")]))
    );
    let (next_retry_at, last_error_kind): (Option<OffsetDateTime>, Option<String>) =
        sqlx::query_as(
            "SELECT next_retry_at, last_error_kind FROM listing_source_urls WHERE url = $1",
        )
        .bind(waiting_url)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((next_retry_at, last_error_kind), (None, None));
    let events = repository.get_review(review_id).await.unwrap().events;
    let (approval_event, restore_event) = (&events[events.len() - 2], &events[events.len() - 1]);
    assert_eq!(
        (approval_event.actor, approval_event.action),
        (ReviewActor::AgentReviewer, ReviewEventAction::Approved)
    );
    assert_eq!(
        (restore_event.actor, restore_event.action),
        (ReviewActor::Human, ReviewEventAction::Restored)
    );
    assert_eq!(
        restore_event.notes.as_deref(),
        Some("agent schema misses sold pages")
    );
    assert_eq!(
        restore_event.details,
        json!({
            "restored_event_id": approval_event.event_id,
            "replaced_product_schemas": [schema("h1")],
            "replaced_removed_page_schemas": [],
        })
    );
}

/// A restore never overwrites a newer change: once the live schemas changed after the approval,
/// it is refused and changes nothing. An approval that replaced no live list restores to none.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_restoring_over_a_newer_live_change_and_restore_no_list_to_none() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (edited_source, _) = listing_source_on(&pool, "edited.example").await;
    let edited_review = pending_review(
        &repository,
        &edited_source,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    approve_as_human(&repository, edited_review).await;
    repository
        .update_schema_field(
            edited_review,
            ReviewActor::Human,
            version(&repository, edited_review).await,
            0,
            "title",
            Some(rule("h1.live")),
        )
        .await
        .unwrap();

    let refused = repository
        .restore_approval(edited_review, ReviewActor::Human, None)
        .await;

    assert!(
        matches!(
            refused,
            Err(ReviewRepositoryError::LiveConfigurationChangedSinceApproval(id)) if id == edited_review
        ),
        "{refused:?}"
    );
    let mut live_edit = schema("h1");
    live_edit.title = rule("h1.live");
    assert_eq!(
        live_schemas(&pool, &edited_source).await,
        Some(vec![live_edit])
    );

    let (first_source, _) = listing_source_on(&pool, "first.example").await;
    let first_review = pending_review(
        &repository,
        &first_source,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    approve_as_human(&repository, first_review).await;
    repository
        .restore_approval(first_review, ReviewActor::Human, None)
        .await
        .unwrap();

    assert_eq!(live_schemas(&pool, &first_source).await, None);
}

/// The removed-page templates are part of what an approval made live: once another writer
/// stored a template after the approval, a restore is refused and leaves both lists alone.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_restoring_once_the_stored_removed_page_templates_changed() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    set_live_schemas(&pool, &listing_source_id, &[schema("h1.old")]).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    approve_as_human(&repository, review_id).await;
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!([notice_template("sold")]))
    .execute(&pool)
    .await
    .unwrap();

    let refused = repository
        .restore_approval(review_id, ReviewActor::Human, None)
        .await;

    assert!(
        matches!(
            refused,
            Err(ReviewRepositoryError::LiveConfigurationChangedSinceApproval(id)) if id == review_id
        ),
        "{refused:?}"
    );
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1")])
    );
    assert_eq!(
        stored_removed_page_schemas(&pool, &listing_source_id).await,
        Some(json!([notice_template("sold")]))
    );
    assert_eq!(
        trail(&repository, review_id).await,
        vec![(ReviewActor::Human, ReviewEventAction::Approved)]
    );
}

/// A restore of a URL-pattern approval brings back the domain's decision and classifies the
/// domain's stored URLs with it again, as the approval did with its own.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_restore_the_url_pattern_an_approval_replaced() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, domain_id) = listing_source(&pool).await;
    sqlx::query(
        "UPDATE listing_source_domains SET url_pattern = '/old/', url_pattern_state = 'MATCHED' \
         WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    for (url, class) in [
        ("https://example.com/old/1", "product"),
        ("https://example.com/new/1", "other"),
    ] {
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, domain_id, url, url_class, raw_source_record_key) \
             VALUES ($1, $2, $3, $4, $3)",
        )
        .bind(listing_source_id.as_uuid())
        .bind(domain_id.as_uuid())
        .bind(url)
        .bind(class)
        .execute(&pool)
        .await
        .unwrap();
    }
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "manual_url_pattern_required",
            UrlPatternDecision::inferred(Some(&Regex::new("/new/").unwrap())),
            &["https://example.com/new/1".to_owned()],
            Some(&Regex::new("/old/").unwrap()),
        )
        .await
        .unwrap();
    let classes = || async {
        sqlx::query_as::<_, (String, String)>(
            "SELECT url, url_class FROM listing_source_urls WHERE domain_id = $1 ORDER BY url",
        )
        .bind(domain_id.as_uuid())
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let decision = || async {
        sqlx::query_as::<_, (Option<String>, String)>(
            "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    };

    approve_as_human(&repository, review_id).await;
    assert_eq!(
        decision().await,
        (Some("/new/".to_owned()), "MATCHED".to_owned())
    );
    repository
        .restore_approval(review_id, ReviewActor::Human, None)
        .await
        .unwrap();

    assert_eq!(
        decision().await,
        (Some("/old/".to_owned()), "MATCHED".to_owned())
    );
    assert_eq!(
        classes().await,
        vec![
            ("https://example.com/new/1".to_owned(), "other".to_owned()),
            ("https://example.com/old/1".to_owned(), "product".to_owned()),
        ]
    );
    assert_eq!(
        trail(&repository, review_id).await,
        vec![
            (ReviewActor::Human, ReviewEventAction::Approved),
            (ReviewActor::Human, ReviewEventAction::Restored),
        ]
    );
}

/// The domain of a shop whose name the keyword heuristic reads as a category.
const SHOP: &str = "antique-shop.example";

/// The URL of `path` on [`SHOP`].
fn shop_url(path: &str) -> String {
    format!("https://{SHOP}{path}")
}

const ITEM_PATHS: [&str; 3] = ["/item/oak-chair", "/item/pine-table", "/item/brass-lamp"];
const OTHER_PATHS: [&str; 3] = ["/item-category/chairs", "/shop", "/impressum"];

/// A pending, undecided URL-pattern review of [`SHOP`] whose sample holds its item pages and its
/// other pages, the crawl that stored them having stored them unclassified.
async fn shop_url_pattern_review(
    pool: &PgPool,
    repository: &CrawlerReviewRepository,
) -> (CrawlerDomainId, CrawlerReviewId) {
    let (listing_source_id, domain_id) = listing_source_on(pool, SHOP).await;
    let sample: Vec<String> = ITEM_PATHS
        .iter()
        .chain(&OTHER_PATHS)
        .map(|path| shop_url(path))
        .collect();
    for url in &sample {
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, domain_id, url, url_class, raw_source_record_key) \
             VALUES ($1, $2, $3, 'other', $3)",
        )
        .bind(listing_source_id.as_uuid())
        .bind(domain_id.as_uuid())
        .bind(url)
        .execute(pool)
        .await
        .unwrap();
    }
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &sample,
            None,
        )
        .await
        .unwrap();
    (domain_id, review_id)
}

fn label(path: &str, label: SampleUrlLabel) -> SampleLabelInput {
    SampleLabelInput {
        url: shop_url(path),
        label,
        reason: "seen on the page".to_owned(),
    }
}

/// The shop's item pages labelled `PRODUCT` and its other pages `NOT_PRODUCT`.
fn shop_labels() -> Vec<SampleLabelInput> {
    ITEM_PATHS
        .iter()
        .map(|path| label(path, SampleUrlLabel::Product))
        .chain(
            OTHER_PATHS
                .iter()
                .map(|path| label(path, SampleUrlLabel::NotProduct)),
        )
        .collect()
}

/// Stages `pattern` as the decision of the review at `expected_candidate_version` as `actor`, and
/// returns the candidate version after it.
async fn decide(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    expected_candidate_version: i64,
    pattern: &str,
) -> i64 {
    repository
        .update_candidate_payload(
            review_id,
            actor,
            expected_candidate_version,
            json!({ "decision": { "type": "PATTERN", "value": pattern }, "current_pattern": null }),
        )
        .await
        .unwrap()
}

async fn candidate_sample_labels(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> serde_json::Value {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .review
        .candidate_payload["sample_labels"]
        .clone()
}

/// Only the author agent and humans label a pending review's sample, each label change based on
/// the candidate version it read, bumping it and recorded as the candidate edit of its actor, the
/// authoring change the four-eyes rule counts. The reviewer agent labels nothing, a label names
/// only a URL of the review's domain, and a decided review's labels stay.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_let_only_the_author_agent_and_humans_label_the_sample_of_a_pending_review() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (_, review_id) = shop_url_pattern_review(&pool, &repository).await;

    let by_reviewer = repository
        .set_sample_labels(review_id, ReviewActor::AgentReviewer, 1, shop_labels())
        .await;
    let outside = repository
        .set_sample_labels(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            vec![SampleLabelInput {
                url: "https://elsewhere.example/item/oak-chair".to_owned(),
                ..label("/item/oak-chair", SampleUrlLabel::Product)
            }],
        )
        .await;
    let by_author = repository
        .set_sample_labels(review_id, ReviewActor::AgentAuthor, 1, shop_labels())
        .await
        .unwrap();
    let stale = repository
        .set_sample_labels(review_id, ReviewActor::AgentAuthor, 1, shop_labels())
        .await;
    let mut relabelled = shop_labels();
    relabelled.pop();
    let by_human = repository
        .set_sample_labels(review_id, ReviewActor::Human, by_author, relabelled)
        .await
        .unwrap();
    decide(
        &repository,
        review_id,
        ReviewActor::Human,
        by_human,
        "/item/[^/]+$",
    )
    .await;
    approve_as_human(&repository, review_id).await;
    let decided_version = version(&repository, review_id).await;
    let by_author_after = repository
        .set_sample_labels(
            review_id,
            ReviewActor::AgentAuthor,
            decided_version,
            shop_labels(),
        )
        .await;
    let by_human_after = repository
        .set_sample_labels(
            review_id,
            ReviewActor::Human,
            decided_version,
            shop_labels(),
        )
        .await;

    assert!(
        matches!(
            by_reviewer,
            Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates)
        ),
        "{by_reviewer:?}"
    );
    assert!(
        matches!(
            &outside,
            Err(ReviewRepositoryError::SampleLabelUrlNotAllowed(url))
                if url == "https://elsewhere.example/item/oak-chair"
        ),
        "{outside:?}"
    );
    assert_eq!((by_author, by_human), (2, 3));
    assert!(
        matches!(
            stale,
            Err(ReviewRepositoryError::StaleCandidateVersion {
                expected: 1,
                current: 2,
                ..
            })
        ),
        "{stale:?}"
    );
    assert!(
        matches!(
            by_author_after,
            Err(ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(_))
        ),
        "{by_author_after:?}"
    );
    assert!(
        matches!(by_human_after, Err(ReviewRepositoryError::NotPending(_))),
        "{by_human_after:?}"
    );
    let detail = repository.get_review(review_id).await.unwrap();
    let labels = detail.review.candidate_payload["sample_labels"]["labels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(labels.len(), 5, "the human's labels are the candidate's");
    let edits: Vec<_> = detail
        .events
        .iter()
        .map(|event| {
            (
                event.actor,
                event.action,
                event.candidate_version,
                event.details["sample_labels"].clone(),
            )
        })
        .collect();
    assert_eq!(
        edits[..2],
        [
            (
                ReviewActor::AgentAuthor,
                ReviewEventAction::CandidateEdited,
                2,
                json!({ "product": 3, "not_product": 3 }),
            ),
            (
                ReviewActor::Human,
                ReviewEventAction::CandidateEdited,
                3,
                json!({ "product": 3, "not_product": 2 }),
            ),
        ]
    );
    assert_eq!(
        trail(&repository, review_id).await[2..],
        [
            (ReviewActor::Human, ReviewEventAction::CandidateEdited),
            (ReviewActor::Human, ReviewEventAction::Approved),
        ]
    );
    assert!(ReviewEventAction::CHANGES_CANDIDATE.contains(&ReviewEventAction::CandidateEdited));
}

/// The labels change only through the sample labels: a decision edit keeps them, and one that
/// names other labels is refused rather than losing them.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_sample_labels_when_the_decision_changes() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (_, review_id) = shop_url_pattern_review(&pool, &repository).await;
    let labelled = repository
        .set_sample_labels(review_id, ReviewActor::AgentAuthor, 1, shop_labels())
        .await
        .unwrap();
    let labels = candidate_sample_labels(&repository, review_id).await;

    let decided = decide(
        &repository,
        review_id,
        ReviewActor::AgentAuthor,
        labelled,
        "/item/[^/]+$",
    )
    .await;
    let with_other_labels = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            decided,
            json!({
                "decision": { "type": "PATTERN", "value": "/item/" },
                "current_pattern": null,
                "sample_labels": { "labelled_at": "2026-01-01T00:00:00Z", "labels": [] },
            }),
        )
        .await;
    let with_the_same_labels = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            decided,
            json!({
                "decision": { "type": "PATTERN", "value": "/item/" },
                "current_pattern": null,
                "sample_labels": labels,
            }),
        )
        .await;

    assert_eq!(decided, labelled + 1);
    assert!(
        matches!(
            with_other_labels,
            Err(ReviewRepositoryError::SampleLabelsChangedThroughCandidate)
        ),
        "{with_other_labels:?}"
    );
    assert_eq!(with_the_same_labels.ok(), Some(decided + 1));
    assert_eq!(
        candidate_sample_labels(&repository, review_id).await,
        labels
    );
}

/// In `agent` mode the reviewer agent approves a URL pattern only once it follows the author's
/// labels, never on what the crawler guesses: the approval classifies the domain's URLs by the
/// pattern alone and keeps the labels it was accepted on.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_agent_url_pattern_approval_only_when_the_decision_follows_the_labels() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20)
        .with_url_classifier(UrlClassifier::for_authoring(CrawlerAuthoringMode::Agent));
    let (domain_id, review_id) = shop_url_pattern_review(&pool, &repository).await;
    let reviewer_approves = |expected_candidate_version: i64| {
        let repository = repository.clone();
        async move {
            repository
                .approve_review(
                    review_id,
                    ReviewActor::AgentReviewer,
                    approval(expected_candidate_version),
                )
                .await
        }
    };

    let undecided = reviewer_approves(1).await;
    let too_broad = decide(&repository, review_id, ReviewActor::AgentAuthor, 1, "/item").await;
    let unlabelled = reviewer_approves(too_broad).await;
    let labelled = repository
        .set_sample_labels(
            review_id,
            ReviewActor::AgentAuthor,
            too_broad,
            shop_labels(),
        )
        .await
        .unwrap();
    let takes_a_category = reviewer_approves(labelled).await;
    let fitting = decide(
        &repository,
        review_id,
        ReviewActor::AgentAuthor,
        labelled,
        "/item/[^/]+$",
    )
    .await;
    let relied_on = candidate_sample_labels(&repository, review_id).await;
    reviewer_approves(fitting).await.unwrap();

    assert_eq!(
        failed_checks(undecided),
        vec![FailedApprovalCheck::CandidateUndecided]
    );
    assert_eq!(
        failed_checks(unlabelled),
        vec![FailedApprovalCheck::TooFewLabels {
            product: 0,
            not_product: 0,
            minimum: 3,
        }]
    );
    assert_eq!(
        failed_checks(takes_a_category),
        vec![FailedApprovalCheck::NotProductLabelMatched {
            urls: vec![shop_url("/item-category/chairs")],
        }]
    );
    let mut classes: Vec<(String, String)> =
        sqlx::query_as("SELECT url, url_class FROM listing_source_urls WHERE domain_id = $1")
            .bind(domain_id.as_uuid())
            .fetch_all(&pool)
            .await
            .unwrap();
    classes.sort();
    let mut expected: Vec<(String, String)> = ITEM_PATHS
        .iter()
        .map(|path| (shop_url(path), "product".to_owned()))
        .chain(
            OTHER_PATHS
                .iter()
                .map(|path| (shop_url(path), "other".to_owned())),
        )
        .collect();
    expected.sort();
    assert_eq!(classes, expected);
    let events = repository.get_review(review_id).await.unwrap().events;
    let approved = events.last().unwrap();
    assert_eq!(
        (approved.actor, approved.action),
        (ReviewActor::AgentReviewer, ReviewEventAction::Approved)
    );
    assert_eq!(
        approved.details["sample_labels"], relied_on,
        "the approval keeps the labels it was accepted on"
    );
    assert!(approved.details.get("url_sample_evaluation").is_none());
}

/// A label change is an authoring change: an agent whose own candidate edit changed the labels of
/// the version it approves is refused, however consistent the labels. No route lets the reviewer
/// agent label, so its label change is recorded here as the event the labels record.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_the_approval_of_an_agent_that_changed_the_labels_it_approves() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (_, review_id) = shop_url_pattern_review(&pool, &repository).await;
    let labelled = repository
        .set_sample_labels(review_id, ReviewActor::Human, 1, shop_labels())
        .await
        .unwrap();
    let decided = decide(
        &repository,
        review_id,
        ReviewActor::Human,
        labelled,
        "/item/[^/]+$",
    )
    .await;
    sqlx::query(
        "INSERT INTO crawler_review_events \
         (event_id, review_id, actor, action, candidate_version, details) \
         VALUES ($1, $2, 'AGENT_REVIEWER', 'CANDIDATE_EDITED', $3, \
                 '{\"sample_labels\": {\"product\": 3, \"not_product\": 3}}')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(review_id.as_uuid())
    .bind(labelled)
    .execute(&pool)
    .await
    .unwrap();

    let own_labels = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(decided))
        .await;

    assert_eq!(
        failed_checks(own_labels),
        vec![FailedApprovalCheck::ApproverIsAuthor]
    );
}

/// The reviewer agent only judges: every change of what a review approves, its candidate, a
/// schema field, a removed-page declaration or a page, is refused before anything is fetched or
/// stored, and records nothing; its needs-repair verdict is taken.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_every_candidate_change_of_the_reviewer_agent_without_an_event() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    let added_url = url::Url::parse("https://example.com/products/table").unwrap();
    let added = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::AgentAuthor,
            &added_url,
            &added_url,
            &shop_page(added_url.as_str()),
        )
        .await
        .unwrap();
    let other_url = url::Url::parse("https://example.com/products/other").unwrap();

    let refusals = [
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentReviewer,
                1,
                json!({ "schemas": [schema("h1.reviewer")] }),
            )
            .await
            .map(|_| ()),
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentReviewer,
                1,
                json!({ "schemas": [schema("h1")], "removed_page_schemas": [notice_template("sold")] }),
            )
            .await
            .map(|_| ()),
        repository
            .update_schema_field(
                review_id,
                ReviewActor::AgentReviewer,
                1,
                0,
                "title",
                Some(rule("h1.reviewer")),
            )
            .await
            .map(|_| ()),
        repository
            .check_reviewer_page_target(review_id, ReviewActor::AgentReviewer, &other_url)
            .await,
        repository
            .add_reviewer_page(
                review_id,
                ReviewActor::AgentReviewer,
                &other_url,
                &other_url,
                &shop_page(other_url.as_str()),
            )
            .await
            .map(|_| ()),
        repository
            .remove_reviewer_page(added.review_page_id, ReviewActor::AgentReviewer)
            .await,
    ];
    repository
        .request_repair(
            review_id,
            ReviewActor::AgentReviewer,
            Some("the table page needs its own schema"),
        )
        .await
        .unwrap();

    for refusal in &refusals {
        assert!(
            matches!(
                refusal,
                Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates)
            ),
            "{refusals:?}"
        );
    }
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.candidate_version, 1);
    assert_eq!(detail.pages.len(), 2);
    assert_eq!(
        trail(&repository, review_id).await,
        vec![
            (ReviewActor::AgentAuthor, ReviewEventAction::PageAdded),
            (
                ReviewActor::AgentReviewer,
                ReviewEventAction::RepairRequested
            ),
        ]
    );
}

/// The author agent stages for the reviewer agent or a human to decide: its approval and
/// rejection are refused whatever the review holds, and record nothing.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_every_verdict_of_the_author_agent() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;

    let approved = repository
        .approve_review(review_id, ReviewActor::AgentAuthor, approval(1))
        .await;
    let rejected = repository
        .reject_review(review_id, ReviewActor::AgentAuthor, Some("not a shop page"))
        .await;

    for refusal in [approved, rejected] {
        assert!(
            matches!(refusal, Err(ReviewRepositoryError::AuthorMayNotDecide)),
            "{refusal:?}"
        );
    }
    assert_eq!(
        repository
            .get_review(review_id)
            .await
            .unwrap()
            .review
            .status,
        STATUS_PENDING_REVIEW
    );
    assert!(trail(&repository, review_id).await.is_empty());
}

/// Four eyes: the reviewer agent approves the version the author agent staged once it evaluated
/// it live; an edit the author makes after that evaluation is approved only as the new version,
/// on a new evaluation.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_reviewer_approval_only_of_the_author_version_it_evaluated() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1.wrong")],
        vec![page("chair")],
    )
    .await;
    let staged = repository
        .update_schema_field(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            0,
            "title",
            Some(rule("h2")),
        )
        .await
        .unwrap();
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    let edited_after_evaluation = repository
        .update_schema_field(
            review_id,
            ReviewActor::AgentAuthor,
            staged,
            0,
            "title",
            Some(rule("h1")),
        )
        .await
        .unwrap();

    let evaluated_version = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(staged))
        .await;
    let unevaluated_version = repository
        .approve_review(
            review_id,
            ReviewActor::AgentReviewer,
            approval(edited_after_evaluation),
        )
        .await;
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(
            review_id,
            ReviewActor::AgentReviewer,
            approval(edited_after_evaluation),
        )
        .await
        .unwrap();

    assert!(
        matches!(
            evaluated_version,
            Err(ReviewRepositoryError::StaleCandidateVersion { expected, current, .. })
                if expected == staged && current == edited_after_evaluation
        ),
        "{evaluated_version:?}"
    );
    assert_eq!(
        failed_checks(unevaluated_version),
        vec![FailedApprovalCheck::LiveMatrixMissing]
    );
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1")])
    );
    assert_eq!(
        trail(&repository, review_id).await,
        vec![
            (
                ReviewActor::AgentAuthor,
                ReviewEventAction::SchemaFieldEdited
            ),
            (
                ReviewActor::AgentAuthor,
                ReviewEventAction::SchemaFieldEdited
            ),
            (ReviewActor::AgentReviewer, ReviewEventAction::Approved),
        ]
    );
}

/// No agent approves its own work: an agent that changed the candidate version it approves, or
/// its pages, is refused with `APPROVER_IS_AUTHOR` however well the version passes the guard,
/// while a human approves the same version. No route lets the reviewer agent change a candidate,
/// so its change is recorded here as an event of its own.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_an_agent_approval_of_a_version_the_approving_agent_changed() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_review(
        &repository,
        &listing_source_id,
        "manual_schema_required",
        &[schema("h1")],
        vec![page("chair")],
    )
    .await;
    for action in ["PAGE_REMOVED", "SCHEMA_FIELD_EDITED"] {
        sqlx::query(
            "INSERT INTO crawler_review_events \
             (event_id, review_id, actor, action, candidate_version, details) \
             VALUES ($1, $2, 'AGENT_REVIEWER', $3, 1, '{}')",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(review_id.as_uuid())
        .bind(action)
        .execute(&pool)
        .await
        .unwrap();
    }
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;

    let own_change = repository
        .approve_review(review_id, ReviewActor::AgentReviewer, approval(1))
        .await;

    assert_eq!(
        failed_checks(own_change),
        vec![FailedApprovalCheck::ApproverIsAuthor]
    );
    assert_eq!(
        repository
            .get_review(review_id)
            .await
            .unwrap()
            .review
            .status,
        STATUS_PENDING_REVIEW
    );
    repository
        .approve_review(review_id, ReviewActor::Human, approval(1))
        .await
        .unwrap();
    assert_eq!(
        live_schemas(&pool, &listing_source_id).await,
        Some(vec![schema("h1")])
    );
}
