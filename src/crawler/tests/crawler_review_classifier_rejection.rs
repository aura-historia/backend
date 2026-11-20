//! A rejection resting on the page classifier's verdicts
//! (`docs/crawler/extraction-and-review.md#page-classifier`): the console admits it only on a
//! fresh `manual_schema_required` review of triggering pages whose every page the verdicts rule
//! out as a product, never on a page that captured a product or whose rejection a human revoked;
//! the agents' queue marks the reviews the classifier judges, the event trail keeps the verdicts,
//! and the non-product loop guard neither counts such a rejection nor lets it break a row.

use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, PageClassification, ReviewActor,
    ReviewEventAction, STATUS_REJECTED, SchemaReviewPageInput, SortDirection,
};
use crawler::review::repository::{
    ClassifierRejectionRefusal, CrawlerReviewRepository, PendingReviewQuery, ReviewRepositoryError,
};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::{CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

const MANUAL_SCHEMA_REASON: &str = "manual_schema_required";
const CATEGORY_URL: &str = "https://www.example.com/antique/chairs/";
const CATEGORY_HTML: &str = "<html><body><h1>Chairs</h1><ul><li>Chair one £100</li><li>Chair two £200</li></ul></body></html>";

fn schema() -> ProductCssSelectorSchema {
    let rule = |selector: &str| ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    };
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: Some(rule("h1")),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: Some(rule("span.state")),
        images: None,
        raw_attributes: Default::default(),
    }
}

/// A repository of `agent` authoring mode, as the crawler builds it for the review agents.
fn agent_repository(pool: &PgPool) -> CrawlerReviewRepository {
    CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(AgentDecisionPolicy::new(
        CrawlerAuthoringMode::Agent,
        AgentLimits::default(),
    ))
}

async fn listing_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, 'Classified source', $2, TRUE, 'https://www.example.com/', 'example.com', \
                 NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(format!("classified-{}", listing_source_id.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    listing_source_id
}

/// Opens, or joins, the source's pending `manual_schema_required` review for `url` as a page of
/// `role`, as the scraper does for a page that fails every live schema.
async fn open_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
    url: &str,
    role: &str,
) -> CrawlerReviewId {
    repository
        .create_schema_review(
            listing_source_id,
            MANUAL_SCHEMA_REASON,
            &[schema()],
            vec![SchemaReviewPageInput {
                url: url.to_owned(),
                role: role.to_owned(),
                raw_html: CATEGORY_HTML.to_owned(),
            }],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap()
}

async fn triggering_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
    url: &str,
) -> CrawlerReviewId {
    open_review(
        repository,
        listing_source_id,
        url,
        PAGE_ROLE_TRIGGERING_GENERATION_PAGE,
    )
    .await
}

async fn page_ids(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Vec<CrawlerReviewPageId> {
    repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| page.review_page_id)
        .collect()
}

fn classification(pages: &[CrawlerReviewPageId], verdict: &str) -> PageClassification {
    serde_json::from_value(json!({
        "pages": pages.iter().map(|page| json!({
            "review_page_id": page.to_string(),
            "verdict": verdict,
            "reason": "Lists several chairs with prices.",
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

/// The reviewer agent's rejection of `review_id` on `verdict` for each of its pages, as the
/// review agent runner sends it.
async fn classifier_reject(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    verdict: &str,
) -> Result<(), ReviewRepositoryError> {
    let pages = page_ids(repository, review_id).await;
    repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("Page classifier: LISTING"),
            Some(&classification(&pages, verdict)),
        )
        .await
}

fn refusal(result: Result<(), ReviewRepositoryError>) -> ClassifierRejectionRefusal {
    match result {
        Err(ReviewRepositoryError::ClassifierRejectionRefused { refusal, .. }) => refusal,
        other => panic!("expected a refused classifier rejection: {other:?}"),
    }
}

async fn classification_due(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> bool {
    repository
        .list_pending_reviews(PendingReviewQuery::default())
        .await
        .unwrap()
        .reviews
        .into_iter()
        .find(|review| review.review_id == review_id)
        .is_some_and(|review| review.classification_due)
}

async fn captured_url(pool: &PgPool, listing_source_id: &ListingSourceId, url: &str) {
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, url, url_class, raw_source_record_key, last_source_listing_id) \
         VALUES ($1, $2, 'product', $2, 'SKU-1')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url)
    .execute(pool)
    .await
    .unwrap();
}

async fn held_for_a_human(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> bool {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .events
        .iter()
        .any(|event| event.action == ReviewEventAction::HandedToHuman)
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_reject_a_fresh_review_of_a_listing_page_and_keep_the_verdicts() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;
    let since = OffsetDateTime::now_utc() - time::Duration::minutes(1);
    assert!(classification_due(&repository, review_id).await);

    classifier_reject(&repository, review_id, "LISTING")
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_REJECTED);
    let rejection = detail.events.last().expect("rejection event");
    assert_eq!(rejection.actor, ReviewActor::AgentReviewer);
    assert_eq!(
        rejection.details["classification"]["pages"][0]["verdict"],
        "LISTING"
    );
    assert!(
        repository
            .has_rejected_schema_review_for_page(
                &listing_source_id,
                MANUAL_SCHEMA_REASON,
                "http://example.com/antique/chairs"
            )
            .await
            .unwrap()
    );
    let (events, _) = repository
        .list_review_events_since(since, 50, SortDirection::Ascending)
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.review_id == review_id && event.classifier_rejection)
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_of_a_review_someone_worked() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;
    repository
        .request_repair(review_id, ReviewActor::Human, Some("Check the title rule"))
        .await
        .unwrap();

    assert!(!classification_due(&repository, review_id).await);
    assert_eq!(
        refusal(classifier_reject(&repository, review_id, "LISTING").await),
        ClassifierRejectionRefusal::NotClassifiable
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_of_a_first_schema_review() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let review_id = open_review(
        &repository,
        &listing_source_id,
        CATEGORY_URL,
        PAGE_ROLE_PRIMARY,
    )
    .await;

    assert!(!classification_due(&repository, review_id).await);
    assert_eq!(
        refusal(classifier_reject(&repository, review_id, "LISTING").await),
        ClassifierRejectionRefusal::NotClassifiable
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_unless_every_page_is_ruled_out_as_a_product() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;

    for verdict in ["PRODUCT", "REMOVED", "UNSURE"] {
        assert_eq!(
            refusal(classifier_reject(&repository, review_id, verdict).await),
            ClassifierRejectionRefusal::VerdictAllowsProduct,
            "{verdict}"
        );
    }
    let another_page = classification(&[CrawlerReviewPageId::new()], "LISTING");
    assert_eq!(
        refusal(
            repository
                .reject_review(
                    review_id,
                    ReviewActor::AgentReviewer,
                    Some("Page classifier: LISTING"),
                    Some(&another_page),
                )
                .await
        ),
        ClassifierRejectionRefusal::PagesNotMatched
    );
    assert!(classification_due(&repository, review_id).await);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_of_a_page_that_captured_a_product() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    // The crawler stores the page under another spelling than the review page.
    captured_url(
        &pool,
        &listing_source_id,
        "https://example.com/antique/chairs",
    )
    .await;
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;

    assert_eq!(
        refusal(classifier_reject(&repository, review_id, "LISTING").await),
        ClassifierRejectionRefusal::PageCapturedProduct
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_of_the_redirect_target_of_a_captured_product() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    captured_url(
        &pool,
        &listing_source_id,
        "https://www.example.com/item/old-chair",
    )
    .await;
    sqlx::query(
        "UPDATE listing_source_urls \
         SET unsettled_redirect_target = $2, unsettled_redirect_streak = 1 WHERE url = $1",
    )
    .bind("https://www.example.com/item/old-chair")
    .bind(CATEGORY_URL)
    .execute(&pool)
    .await
    .unwrap();
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;

    assert_eq!(
        refusal(classifier_reject(&repository, review_id, "OTHER").await),
        ClassifierRejectionRefusal::PageCapturedProduct
    );
}

/// A human who revoked the rejection of a page decided it may be a product: the classifier
/// never retires it again when it fails every schema and opens a new review.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classifier_rejection_of_a_page_whose_rejection_a_human_revoked() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let first = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;
    classifier_reject(&repository, first, "LISTING")
        .await
        .unwrap();
    repository
        .revoke_rejection(first, ReviewActor::Human, Some("This is a product page"))
        .await
        .unwrap();
    let reopened = triggering_review(
        &repository,
        &listing_source_id,
        "https://example.com/antique/chairs",
    )
    .await;

    assert_eq!(
        refusal(classifier_reject(&repository, reopened, "LISTING").await),
        ClassifierRejectionRefusal::RejectionRevokedBefore
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_classification_sent_by_anyone_but_the_reviewer_agent() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let review_id = triggering_review(&repository, &listing_source_id, CATEGORY_URL).await;
    let pages = page_ids(&repository, review_id).await;

    let refused = repository
        .reject_review(
            review_id,
            ReviewActor::Human,
            None,
            Some(&classification(&pages, "LISTING")),
        )
        .await;

    assert!(
        matches!(
            refused,
            Err(ReviewRepositoryError::ClassificationNeedsReviewerAgent)
        ),
        "{refused:?}"
    );
}

/// The classifier's rejections spend no agent rounds, so they neither count towards the loop
/// guard nor break a row of rejections that do.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_neither_count_nor_break_the_non_product_loop_with_classifier_rejections() {
    let pool = get_postgres_client().await;
    let repository = agent_repository(&pool);
    let listing_source_id = listing_source(&pool).await;
    let url = |n: u32| format!("https://www.example.com/antique/category-{n}/");

    for n in 1..=2 {
        let review_id = triggering_review(&repository, &listing_source_id, &url(n)).await;
        classifier_reject(&repository, review_id, "LISTING")
            .await
            .unwrap();
    }
    let after_classifier_rejections =
        triggering_review(&repository, &listing_source_id, &url(3)).await;
    assert!(!held_for_a_human(&repository, after_classifier_rejections).await);

    repository
        .reject_review(
            after_classifier_rejections,
            ReviewActor::AgentReviewer,
            Some("A category page"),
            None,
        )
        .await
        .unwrap();
    let classified = triggering_review(&repository, &listing_source_id, &url(4)).await;
    assert!(!held_for_a_human(&repository, classified).await);
    classifier_reject(&repository, classified, "OTHER")
        .await
        .unwrap();
    let worked = triggering_review(&repository, &listing_source_id, &url(5)).await;
    repository
        .reject_review(
            worked,
            ReviewActor::AgentReviewer,
            Some("A category page"),
            None,
        )
        .await
        .unwrap();

    let next = triggering_review(&repository, &listing_source_id, &url(6)).await;
    assert!(held_for_a_human(&repository, next).await);
}
