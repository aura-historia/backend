//! The review agent's decisions and a human's restore: the agent approves and rejects only while
//! its decisions are `guarded`, every approval names the candidate version it was based on, an
//! agent approval passes the approval guard on the server's own live evaluation, the agent's
//! decisions of an hour are capped from the event trail, and a human restores what an approval
//! replaced while it is still live.

use crawler::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
use crawler::review::approval_guard::{FailedApprovalCheck, GuardedPage};
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, ReviewActor, ReviewEventAction, STATUS_APPROVED, STATUS_PENDING_REVIEW,
    SchemaReviewPageInput, UrlPatternDecision,
};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
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

/// With agent decisions off the agent stages candidates and leaves every decision to a human.
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
            ReviewActor::Agent,
            1,
            json!({ "schemas": [schema("h1.agent")] }),
        )
        .await
        .unwrap();
    repository
        .request_repair(
            review_id,
            ReviewActor::Agent,
            Some("title selector too loose"),
        )
        .await
        .unwrap();
    let approved = repository
        .approve_review(review_id, ReviewActor::Agent, approval(edited))
        .await;
    let rejected = repository
        .reject_review(review_id, ReviewActor::Agent, Some("not a product page"))
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
            (ReviewActor::Agent, ReviewEventAction::CandidateEdited),
            (ReviewActor::Agent, ReviewEventAction::RepairRequested),
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

    for actor in [ReviewActor::Human, ReviewActor::Agent] {
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
        .approve_review(review_id, ReviewActor::Agent, approval(1))
        .await;
    let acknowledged = repository
        .approve_review(
            review_id,
            ReviewActor::Agent,
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
        .approve_review(review_id, ReviewActor::Agent, approval(1))
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
            ReviewActor::Agent,
            1,
            json!({ "schemas": [schema("h1"), schema("h2")] }),
        )
        .await
        .unwrap();
    let dropped = repository
        .approve_review(review_id, ReviewActor::Agent, approval(edited))
        .await;
    store_live_matrix(
        &repository,
        review_id,
        OffsetDateTime::now_utc() - Duration::from_secs(2 * 60 * 60),
    )
    .await;
    let expired = repository
        .approve_review(review_id, ReviewActor::Agent, approval(edited))
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
        vec![(ReviewActor::Agent, ReviewEventAction::CandidateEdited)]
    );

    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(review_id, ReviewActor::Agent, approval(edited))
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
        (ReviewActor::Agent, ReviewEventAction::Approved)
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
        .approve_review(review_id, ReviewActor::Agent, approval(1))
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
        .approve_review(review_id, ReviewActor::Agent, approval(1))
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
            .reject_review(*review_id, ReviewActor::Agent, Some("not a shop page"))
            .await
            .unwrap();
    }
    let rejection = repository
        .reject_review(reviews[2], ReviewActor::Agent, Some("not a shop page"))
        .await;
    let approval_after_restart = guarded(&pool, 2)
        .approve_review(reviews[3], ReviewActor::Agent, approval(1))
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
            ReviewActor::Agent,
            1,
            json!({ "schemas": [schema("h1")], "removed_page_schemas": [] }),
        )
        .await
        .unwrap();
    store_live_matrix(&repository, review_id, OffsetDateTime::now_utc()).await;
    repository
        .approve_review(review_id, ReviewActor::Agent, approval(edited))
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

    let by_agent = repository
        .restore_approval(review_id, ReviewActor::Agent, None)
        .await;
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

    assert!(
        matches!(by_agent, Err(ReviewRepositoryError::RestoreNeedsHuman)),
        "{by_agent:?}"
    );
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
        (ReviewActor::Agent, ReviewEventAction::Approved)
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

/// The agent approves a URL pattern only when it is decided and classifies the review's sample
/// as the crawler knows it: every URL it captured a product from as a product, and none of the
/// category, imprint or info URLs.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_agent_url_pattern_approval_only_when_it_fits_the_classified_sample() {
    let pool = get_postgres_client().await;
    let repository = guarded(&pool, 20);
    let (listing_source_id, domain_id) = listing_source(&pool).await;
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, url_class, raw_source_record_key, last_source_listing_id) \
         VALUES ($1, $2, 'https://example.com/product/chair', 'other', 'chair', 'chair')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &[
                "https://example.com/product/chair".to_owned(),
                "https://example.com/product/table".to_owned(),
                "https://example.com/product-category/chairs".to_owned(),
            ],
            None,
        )
        .await
        .unwrap();
    let decide = |pattern: &str, expected_candidate_version: i64| {
        let repository = repository.clone();
        let pattern = pattern.to_owned();
        async move {
            repository
                .update_candidate_payload(
                    review_id,
                    ReviewActor::Agent,
                    expected_candidate_version,
                    json!({ "decision": { "type": "PATTERN", "value": pattern }, "current_pattern": null }),
                )
                .await
                .unwrap()
        }
    };

    let undecided = repository
        .approve_review(review_id, ReviewActor::Agent, approval(1))
        .await;
    let too_broad = decide("/product", 1).await;
    let matches_a_category = repository
        .approve_review(review_id, ReviewActor::Agent, approval(too_broad))
        .await;
    let fitting = decide("/product/[^/]+$", too_broad).await;
    repository
        .approve_review(review_id, ReviewActor::Agent, approval(fitting))
        .await
        .unwrap();

    assert_eq!(
        failed_checks(undecided),
        vec![FailedApprovalCheck::CandidateUndecided]
    );
    assert_eq!(
        failed_checks(matches_a_category),
        vec![FailedApprovalCheck::NonProductUrlsMatched {
            urls: vec!["https://example.com/product-category/chairs".to_owned()],
        }]
    );
    let url_class: String =
        sqlx::query_scalar("SELECT url_class FROM listing_source_urls WHERE url = $1")
            .bind("https://example.com/product/chair")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(url_class, "product");
}
