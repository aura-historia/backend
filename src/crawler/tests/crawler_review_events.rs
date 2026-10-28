//! The review event trail: every accepted review mutation appends exactly one event, with its
//! actor, in the mutation's own transaction, and a refused mutation appends none.

use crawler::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
use crawler::review::approval_guard::FailedApprovalCheck;
use crawler::review::model::{
    CrawlerReviewEvent, PAGE_ROLE_PRIMARY, ReviewActor, ReviewEventAction, STATUS_PENDING_REVIEW,
    SchemaReviewPageInput, UrlPatternDecision,
};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

const PRODUCT_HTML: &str = "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>";

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

fn url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

/// A repository that takes the review agent's guarded approvals and rejections.
fn deciding_agent_repository(pool: &PgPool) -> CrawlerReviewRepository {
    CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(AgentDecisionPolicy {
        approvals: AgentApprovals::Guarded,
        ..AgentDecisionPolicy::default()
    })
}

/// A ListingSource on `example.com` and its domain.
async fn listing_source(pool: &PgPool) -> (ListingSourceId, CrawlerDomainId) {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Event source', 'event-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'example.com', 'www.example.com')",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    (listing_source_id, domain_id)
}

/// A pending manual product-schema review of the ListingSource over one seed page.
async fn pending_schema_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
) -> CrawlerReviewId {
    repository
        .create_schema_review(
            listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_owned(),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                raw_html: PRODUCT_HTML.to_owned(),
            }],
            json!({}),
        )
        .await
        .unwrap()
}

async fn events(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Vec<CrawlerReviewEvent> {
    repository.get_review(review_id).await.unwrap().events
}

/// Who did what to the review, and the candidate version it left, oldest first.
fn trail(events: &[CrawlerReviewEvent]) -> Vec<(ReviewActor, ReviewEventAction, i64)> {
    events
        .iter()
        .map(|event| (event.actor, event.action, event.candidate_version))
        .collect()
}

async fn candidate_version(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> i64 {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .review
        .candidate_version
}

/// Approves the review's current candidate version as `actor`, as an approver who just read it.
async fn approve(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    notes: Option<&str>,
    incomplete_coverage: IncompleteCoverage,
) -> Result<(), ReviewRepositoryError> {
    let expected_candidate_version = repository.candidate_version(review_id).await?;
    repository
        .approve_review(
            review_id,
            actor,
            Approval {
                expected_candidate_version,
                notes,
                incomplete_coverage,
            },
        )
        .await
}

async fn approve_acknowledging_coverage(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    notes: Option<&str>,
) {
    let Err(ReviewRepositoryError::IncompleteCoverage { coverage, .. }) = approve(
        repository,
        review_id,
        actor,
        notes,
        IncompleteCoverage::Refuse,
    )
    .await
    else {
        panic!("a review without a stored matrix has incomplete coverage");
    };
    approve(
        repository,
        review_id,
        actor,
        notes,
        IncompleteCoverage::Acknowledged(coverage),
    )
    .await
    .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_record_each_accepted_mutation_once_with_its_actor_and_candidate_version() {
    let pool = get_postgres_client().await;
    let repository = deciding_agent_repository(&pool);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let added_url = url("https://example.com/products/2");

    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({
                "schemas": [schema("h1.agent")],
                "removed_page_schemas": [notice_template("sold")],
            }),
        )
        .await
        .unwrap();
    repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            2,
            0,
            "title",
            Some(rule("h1.human")),
        )
        .await
        .unwrap();
    let added = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::AgentAuthor,
            &added_url,
            &added_url,
            PRODUCT_HTML,
        )
        .await
        .unwrap();
    repository
        .remove_reviewer_page(added.review_page_id, ReviewActor::AgentAuthor)
        .await
        .unwrap();
    repository
        .request_repair(review_id, ReviewActor::Human, Some(" fix the price "))
        .await
        .unwrap();
    repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("not a product page"),
        )
        .await
        .unwrap();

    let events = events(&repository, review_id).await;
    assert_eq!(
        trail(&events),
        vec![
            (
                ReviewActor::AgentAuthor,
                ReviewEventAction::CandidateEdited,
                2
            ),
            (ReviewActor::Human, ReviewEventAction::SchemaFieldEdited, 3),
            (ReviewActor::AgentAuthor, ReviewEventAction::PageAdded, 3),
            (ReviewActor::AgentAuthor, ReviewEventAction::PageRemoved, 3),
            (ReviewActor::Human, ReviewEventAction::RepairRequested, 3),
            (ReviewActor::AgentReviewer, ReviewEventAction::Rejected, 3),
        ]
    );
    assert_eq!(
        events[0].details,
        json!({ "removed_page_schemas_changed": true })
    );
    assert_eq!(
        events[1].details,
        json!({ "schema_index": 0, "field": "title" })
    );
    for page_event in &events[2..4] {
        assert_eq!(
            page_event.details,
            json!({ "url": added_url.as_str(), "role": "REVIEWER_ADDED" })
        );
    }
    assert_eq!(events[4].notes.as_deref(), Some("fix the price"));
    assert_eq!(events[5].notes.as_deref(), Some("not a product page"));
    assert!(events.iter().all(|event| event.review_id == review_id));
}

/// An approval keeps the live schema list and stored removed-page templates it replaced and the
/// ones it made live, which a restore needs, and the coverage it was accepted under. It moves the
/// review past the approved candidate version.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_live_configuration_an_approval_replaced_and_its_coverage() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    sqlx::query(
        "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!([schema("h1.old")]))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!([notice_template("sold")]))
    .execute(&pool)
    .await
    .unwrap();
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1.new")], "removed_page_schemas": [] }),
        )
        .await
        .unwrap();

    approve_acknowledging_coverage(
        &repository,
        review_id,
        ReviewActor::Human,
        Some("matches the shop"),
    )
    .await;

    let events = events(&repository, review_id).await;
    assert_eq!(
        trail(&events),
        vec![
            (
                ReviewActor::AgentAuthor,
                ReviewEventAction::CandidateEdited,
                2
            ),
            (ReviewActor::Human, ReviewEventAction::Approved, 3),
        ]
    );
    let approval = &events[1];
    assert_eq!(approval.notes.as_deref(), Some("matches the shop"));
    assert_eq!(
        approval.details,
        json!({
            "coverage": { "covered": 0, "removed": 0, "total": 1, "not_evaluated": 1 },
            "replaced_product_schemas": [schema("h1.old")],
            "replaced_removed_page_schemas": [notice_template("sold")],
            "approved_product_schemas": [schema("h1.new")],
            "approved_removed_page_schemas": [],
        })
    );
}

/// A human's edit through the approved review whose candidate is live rewrites the live list, so
/// its event keeps the list it replaced, like an approval's.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_live_schemas_an_edit_of_the_live_review_replaced() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    approve_acknowledging_coverage(&repository, review_id, ReviewActor::Human, None).await;
    let version = candidate_version(&repository, review_id).await;

    repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            version,
            0,
            "title",
            Some(rule("h1.live")),
        )
        .await
        .unwrap();

    let events = events(&repository, review_id).await;
    assert_eq!(
        trail(&events),
        vec![
            (ReviewActor::Human, ReviewEventAction::Approved, version),
            (
                ReviewActor::Human,
                ReviewEventAction::SchemaFieldEdited,
                version + 1
            ),
        ]
    );
    assert_eq!(events[0].details["replaced_product_schemas"], json!(null));
    assert_eq!(
        events[1].details,
        json!({ "schema_index": 0, "field": "title", "replaced_product_schemas": [schema("h1")] })
    );
}

/// The crawler's own approval of an inferred URL pattern in `auto` mode goes through the same
/// approval, recorded as `SYSTEM`, and keeps the decision it replaced.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_url_pattern_decision_an_approval_replaced() {
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
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            UrlPatternDecision::inferred(Some(&Regex::new("/new/").unwrap())),
            &["https://example.com/new/1".to_owned()],
            Some(&Regex::new("/old/").unwrap()),
        )
        .await
        .unwrap();

    approve(
        &repository,
        review_id,
        ReviewActor::System,
        Some("Applied directly by CRAWLER_AUTHORING_MODE=auto"),
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    let events = events(&repository, review_id).await;
    assert_eq!(
        trail(&events),
        vec![(ReviewActor::System, ReviewEventAction::Approved, 2)]
    );
    assert_eq!(
        events[0].details,
        json!({
            "replaced_url_pattern": { "url_pattern": "/old/", "url_pattern_state": "MATCHED" },
            "approved_url_pattern": { "url_pattern": "/new/", "url_pattern_state": "MATCHED" },
        })
    );
}

/// The agent changes live configuration only through an approval: every change of the approved
/// review whose candidate is live, or of a closed review, is refused as the agent's before any
/// other check, leaves the live schemas and the review as they were and records nothing.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_every_agent_change_of_a_decided_review_without_an_event() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let added_url = url("https://example.com/products/2");
    let added = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &added_url,
            &added_url,
            PRODUCT_HTML,
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, review_id, ReviewActor::Human, None).await;
    let approved = repository.get_review(review_id).await.unwrap();
    let version = approved.review.candidate_version;
    assert!(approved.candidate_is_live);
    let other_url = url("https://example.com/products/3");

    let refusals = [
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentAuthor,
                version,
                json!({ "schemas": [schema("h1.agent")] }),
            )
            .await
            .map(|_| ()),
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentAuthor,
                version,
                json!({
                    "schemas": [schema("h1")],
                    "removed_page_schemas": [notice_template("sold")],
                }),
            )
            .await
            .map(|_| ()),
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentAuthor,
                version - 1,
                json!({ "schemas": [schema("h1.stale")] }),
            )
            .await
            .map(|_| ()),
        repository
            .update_schema_field(
                review_id,
                ReviewActor::AgentAuthor,
                version,
                0,
                "title",
                Some(rule("h1.agent")),
            )
            .await
            .map(|_| ()),
        repository
            .check_reviewer_page_target(review_id, ReviewActor::AgentAuthor, &other_url)
            .await,
        repository
            .add_reviewer_page(
                review_id,
                ReviewActor::AgentAuthor,
                &other_url,
                &other_url,
                PRODUCT_HTML,
            )
            .await
            .map(|_| ()),
        repository
            .remove_reviewer_page(added.review_page_id, ReviewActor::AgentAuthor)
            .await,
    ];

    for refusal in &refusals {
        assert!(
            matches!(
                refusal,
                Err(ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(id)) if *id == review_id
            ),
            "{refusals:?}"
        );
    }
    let after = repository.get_review(review_id).await.unwrap();
    assert_eq!(after.review.candidate_version, version);
    assert_eq!(
        after.review.candidate_payload,
        approved.review.candidate_payload
    );
    assert_eq!(after.pages.len(), approved.pages.len());
    assert!(after.candidate_is_live);
    assert_eq!(
        trail(&after.events),
        vec![
            (ReviewActor::Human, ReviewEventAction::PageAdded, 1),
            (ReviewActor::Human, ReviewEventAction::Approved, version),
        ]
    );

    let rejected_id = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .reject_review(rejected_id, ReviewActor::Human, None)
        .await
        .unwrap();
    let closed_edit = repository
        .update_candidate_payload(
            rejected_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1.agent")] }),
        )
        .await;
    assert!(
        matches!(
            closed_edit,
            Err(ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(id)) if id == rejected_id
        ),
        "{closed_edit:?}"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_record_no_event_for_a_refused_mutation() {
    let pool = get_postgres_client().await;
    let repository = deciding_agent_repository(&pool);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let seed_page_id = repository.get_review_pages(review_id).await.unwrap()[0].review_page_id;
    let foreign_url = url("https://elsewhere.example/products/1");

    let refusals = [
        repository
            .update_candidate_payload(
                review_id,
                ReviewActor::AgentAuthor,
                2,
                json!({ "schemas": [schema("h1.stale")] }),
            )
            .await
            .map(|_| ()),
        repository
            .update_schema_field(
                review_id,
                ReviewActor::AgentAuthor,
                1,
                0,
                "no_such_field",
                None,
            )
            .await
            .map(|_| ()),
        repository
            .add_reviewer_page(
                review_id,
                ReviewActor::AgentAuthor,
                &foreign_url,
                &foreign_url,
                PRODUCT_HTML,
            )
            .await
            .map(|_| ()),
        repository
            .remove_reviewer_page(seed_page_id, ReviewActor::AgentAuthor)
            .await,
        repository
            .request_repair(review_id, ReviewActor::AgentAuthor, Some("  "))
            .await,
        approve(
            &repository,
            review_id,
            ReviewActor::AgentReviewer,
            None,
            IncompleteCoverage::Refuse,
        )
        .await,
    ];

    assert!(
        matches!(
            refusals.as_slice(),
            [
                Err(ReviewRepositoryError::StaleCandidateVersion { .. }),
                Err(ReviewRepositoryError::InvalidSchemaField(_)),
                Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(_)),
                Err(ReviewRepositoryError::NotReviewerAddedPage(_)),
                Err(ReviewRepositoryError::MissingRepairNote),
                Err(ReviewRepositoryError::AgentApprovalRefused { failed_checks, .. }),
            ] if failed_checks.as_slice() == [FailedApprovalCheck::LiveMatrixMissing]
        ),
        "{refusals:?}"
    );
    assert!(events(&repository, review_id).await.is_empty());

    repository
        .reject_review(review_id, ReviewActor::Human, None)
        .await
        .unwrap();
    let closed = [
        repository
            .reject_review(
                review_id,
                ReviewActor::AgentReviewer,
                Some("not a shop page"),
            )
            .await,
        repository
            .request_repair(review_id, ReviewActor::AgentReviewer, Some("fix it"))
            .await,
    ];
    for refusal in closed {
        assert!(
            matches!(refusal, Err(ReviewRepositoryError::NotPending(id)) if id == review_id),
            "{refusal:?}"
        );
    }
    assert_eq!(
        trail(&events(&repository, review_id).await),
        vec![(ReviewActor::Human, ReviewEventAction::Rejected, 1)]
    );
}

/// The event is written in the mutation's transaction: when it cannot be written, the mutation
/// does not happen either.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_roll_back_the_mutation_when_its_event_cannot_be_recorded() {
    let pool = get_postgres_client().await;
    let repository = deciding_agent_repository(&pool);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    sqlx::query(
        "CREATE FUNCTION fail_review_event() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'injected event failure'; END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_review_event BEFORE INSERT ON crawler_review_events \
         FOR EACH ROW EXECUTE FUNCTION fail_review_event()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let edit = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::AgentAuthor,
            1,
            json!({ "schemas": [schema("h1.agent")] }),
        )
        .await;
    let rejection = repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("not a product page"),
        )
        .await;

    assert!(
        matches!(edit, Err(ReviewRepositoryError::Database(_))),
        "{edit:?}"
    );
    assert!(
        matches!(rejection, Err(ReviewRepositoryError::Database(_))),
        "{rejection:?}"
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.candidate_version, 1);
    assert_eq!(
        detail.review.candidate_payload["schemas"][0]["title"]["selector"],
        "h1"
    );
    assert_eq!(detail.review.status, STATUS_PENDING_REVIEW);
    assert!(detail.events.is_empty());
}

/// The trail is append-only, an event is never updated and deleted only with its review, and it
/// stores only the known actors and actions, in `SCREAMING_SNAKE_CASE`.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_trail_append_only_with_known_actors_and_actions() {
    let pool = get_postgres_client().await;
    let repository = deciding_agent_repository(&pool);
    let (listing_source_id, _) = listing_source(&pool).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("not a shop page"),
        )
        .await
        .unwrap();

    let rewrite = sqlx::query("UPDATE crawler_review_events SET actor = 'HUMAN'")
        .execute(&pool)
        .await;
    let delete = sqlx::query("DELETE FROM crawler_review_events")
        .execute(&pool)
        .await;
    let insert = |actor: &'static str, action: &'static str| {
        sqlx::query(
            "INSERT INTO crawler_review_events \
             (event_id, review_id, actor, action, candidate_version, details) \
             VALUES ($1, $2, $3, $4, 1, '{}')",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(review_id.as_uuid())
        .bind(actor)
        .bind(action)
        .execute(&pool)
    };
    let lowercase_actor = insert("agent_reviewer", "REJECTED").await;
    let unknown_actor = insert("AGENT_OBSERVER", "REJECTED").await;
    let unknown_action = insert("AGENT_REVIEWER", "RESTORE").await;

    for refused in [
        rewrite.map(|_| ()),
        delete.map(|_| ()),
        lowercase_actor.map(|_| ()),
        unknown_actor.map(|_| ()),
        unknown_action.map(|_| ()),
    ] {
        assert!(refused.is_err());
    }
    assert_eq!(
        trail(&events(&repository, review_id).await),
        vec![(ReviewActor::AgentReviewer, ReviewEventAction::Rejected, 1)]
    );

    // An event goes with its review, here with the ListingSource the review belongs to.
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
}
