use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, STATUS_APPROVED,
    STATUS_PENDING_REVIEW, SchemaPageReference, SchemaReviewPageInput,
};
use crawler::review::model::{ReviewActor, UrlPatternDecision, UrlPatternReviewCandidate};
use crawler::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crawler::review::repository::{
    Approval, CrawlerReviewRepository, ReviewRepositoryError, SchemaReviewWithStatusInput,
};
use crawler::scraper::candidate_service::{
    FetchFailureRequest, SchemaReviewWait, SchemaReviewWaitRequest, ScraperCandidateService,
    ScraperCandidateServiceImpl,
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
use crawler::spider::candidate_service::{
    AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME, NEEDS_OPERATOR_CRAWL_OUTCOME,
    SpiderCandidateService, SpiderCandidateServiceImpl,
};
use crawler::spider::classification::url_classifier::UrlClassifier;
use crawler::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use crawler::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sqlx::{AssertSqlSafe, PgPool};
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

fn image_rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Attribute { name: "src".into() },
        cardinality: ExtractionCardinality::All,
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
        images: image_rule("img.product"),
        raw_attributes: Default::default(),
    }
}

async fn insert_listing_source(pool: &PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Test source', 'test-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_domain(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    domain: &str,
) -> CrawlerDomainId {
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
    domain_id
}

/// The candidate version a reviewer reads before editing the review's candidate.
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

fn review_pages() -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: "https://example.com/products/1".to_string(),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
    }]
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn fresh_generation_review_page_role_is_persisted() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "fresh_schema_generation",
            schemas: &[schema("h1")],
            pages: vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_string(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_string(),
                raw_html: "<html><body><h1>Title</h1></body></html>".to_string(),
            }],
            validation_summary: json!({}),
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
        .unwrap();

    assert_eq!(review_page_count(&pool, review_id).await, 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_define_database_defaults_for_crawler_object_ids() {
    let pool = get_postgres_client().await;
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::bigint, COUNT(column_default)::bigint \
         FROM information_schema.columns \
         WHERE table_schema = current_schema() \
           AND (table_name, column_name) IN ( \
             ('listing_source_domains', 'domain_id'), \
             ('crawler_reviews', 'review_id'), \
             ('crawler_review_pages', 'review_page_id'), \
             ('crawler_review_urls', 'review_url_id'), \
             ('crawler_review_events', 'event_id') \
           )",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(counts, (5, 0));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_persist_uuid_v7_for_generated_review_page_and_url_ids() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "generated-ids.example.com").await;

    let schema_review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    let schema_detail = repository.get_review(schema_review_id).await.unwrap();

    assert_eq!(
        7,
        schema_detail.review.review_id.as_uuid().get_version_num()
    );
    assert_eq!(
        7,
        schema_detail.pages[0]
            .review_page_id
            .as_uuid()
            .get_version_num()
    );

    let pattern_review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            UrlPatternDecision::inferred(Some(&Regex::new("/product/").unwrap())),
            &["https://generated-ids.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let pattern_detail = repository.get_review(pattern_review_id).await.unwrap();

    assert_eq!(
        7,
        pattern_detail.review.review_id.as_uuid().get_version_num()
    );
    assert_eq!(
        7,
        pattern_detail.urls[0]
            .review_url_id
            .as_uuid()
            .get_version_num()
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_surface_invalid_persisted_v4_review_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let invalid_persisted_review_id =
        Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap();
    sqlx::query(
        "INSERT INTO crawler_reviews (review_id, listing_source_id, artifact_type, status, reason, candidate_payload, validation_summary) \
         VALUES ($1, $2, 'PRODUCT_SCHEMA', 'PENDING_REVIEW', 'corrupt test row', '{}'::jsonb, '{}'::jsonb)",
    )
    .bind(invalid_persisted_review_id)
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.list_reviews(10).await;

    assert!(matches!(result, Err(sqlx::Error::Decode(_))));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_surface_invalid_persisted_v4_review_page_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    let invalid_persisted_page_id =
        Uuid::parse_str("00000000-0000-4000-8000-000000000002").unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_pages (review_page_id, review_id, url, role, html_hash) \
         VALUES ($1, $2, 'https://example.com/products/corrupt', 'SEED', 'corrupt')",
    )
    .bind(invalid_persisted_page_id)
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.get_review_pages(review_id).await;

    assert!(matches!(result, Err(sqlx::Error::Decode(_))));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_surface_invalid_persisted_v4_review_url_id() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "corrupt-review-url.example.com").await;
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            UrlPatternDecision::inferred(Some(&Regex::new("/product/").unwrap())),
            &["https://corrupt-review-url.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let invalid_persisted_url_id = Uuid::parse_str("00000000-0000-4000-8000-000000000003").unwrap();
    sqlx::query(
        "INSERT INTO crawler_review_urls (review_url_id, review_id, url) \
         VALUES ($1, $2, 'https://corrupt-review-url.example.com/product/corrupt')",
    )
    .bind(invalid_persisted_url_id)
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let result = repository.get_review(review_id).await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::Database(sqlx::Error::Decode(_)))
    ));
}

async fn pending_review_count(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    artifact_type: &str,
) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM crawler_reviews
         WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'",
    )
    .bind(listing_source_id.as_uuid())
    .bind(artifact_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn review_page_count(pool: &PgPool, review_id: CrawlerReviewId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_pages WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_url_count(pool: &PgPool, review_id: CrawlerReviewId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_urls WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_count(pool: &PgPool, listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_reviews WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_roll_back_schema_review_when_evidence_insert_fails() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    sqlx::query(
        "CREATE FUNCTION fail_crawler_review_page_insert() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN RAISE EXCEPTION 'injected review-page failure'; END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_crawler_review_page_insert \
         BEFORE INSERT ON crawler_review_pages FOR EACH ROW \
         WHEN (NEW.url = 'https://rollback-review.example/fail') \
         EXECUTE FUNCTION fail_crawler_review_page_insert()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            vec![SchemaReviewPageInput {
                url: "https://rollback-review.example/fail".to_owned(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html></html>".to_owned(),
            }],
            json!({}),
        )
        .await;

    assert!(result.is_err());
    assert_eq!(review_count(&pool, listing_source_id).await, 0);
    sqlx::query("DROP TRIGGER fail_crawler_review_page_insert ON crawler_review_pages")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_crawler_review_page_insert()")
        .execute(&pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn approved_schema_candidate_edit_updates_live_schema_and_audit_payload() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let initial_schema = schema("h1.old");
    let now = OffsetDateTime::now_utc();
    schema_repository
        .insert_product_schema(
            &listing_source_id,
            &ListingSourceProductSchema {
                listing_source_id,
                product_schemas: vec![initial_schema.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "initial_schema_generation",
            schemas: &[initial_schema],
            pages: vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html><body><span class=\"id\">SKU</span><h1 class=\"new\">Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
            }],
            validation_summary: json!({
                "auto_schema_evaluation": {
                    "decision": "APPROVE",
                    "confidence": "HIGH",
                    "approved_by_llm": true,
                    "summary": "ok"
                }
            }),
            status: STATUS_APPROVED,
            notes: Some("Auto-approved by LLM schema evaluator"),
        })
        .await
        .unwrap();

    let updated_schema = schema("h1.new");
    review_repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, review_id).await,
            json!({ "schemas": [updated_schema.clone()] }),
        )
        .await
        .unwrap();

    let live = schema_repository
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.product_schemas, vec![updated_schema.clone()]);

    let detail = review_repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.candidate_payload,
        json!({ "schemas": [updated_schema] })
    );
    let edits = detail.review.validation_summary["manual_schema_edits"]
        .as_array()
        .expect("manual edits should be recorded");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["source"], "review_console");
    assert_eq!(edits[0]["operation"], "approved_schema_live_update");
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_edit_of_approved_review_when_its_candidate_is_no_longer_live() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let older_review_id = review_repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1.first")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&review_repository, older_review_id)
        .await
        .unwrap();
    let newer_review_id = review_repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1.second")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&review_repository, newer_review_id)
        .await
        .unwrap();

    let candidate_edit = review_repository
        .update_candidate_payload(
            older_review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, older_review_id).await,
            json!({ "schemas": [schema("h1.stale-edit")] }),
        )
        .await;
    let field_edit = review_repository
        .update_schema_field(
            older_review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, older_review_id).await,
            0,
            "title",
            Some(rule("h1.stale-field")),
        )
        .await;

    assert!(matches!(
        candidate_edit,
        Err(ReviewRepositoryError::StaleApprovedReview(id)) if id == older_review_id
    ));
    assert!(matches!(
        field_edit,
        Err(ReviewRepositoryError::StaleApprovedReview(id)) if id == older_review_id
    ));
    let candidate_is_live = |review_id| {
        let review_repository = review_repository.clone();
        async move {
            review_repository
                .get_review(review_id)
                .await
                .unwrap()
                .candidate_is_live
        }
    };
    assert!(!candidate_is_live(older_review_id).await);
    assert!(candidate_is_live(newer_review_id).await);
    let live = || async {
        schema_repository
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .unwrap()
            .product_schemas
    };
    assert_eq!(live().await, vec![schema("h1.second")]);
    let older = review_repository.get_review(older_review_id).await.unwrap();
    assert_eq!(
        older.review.candidate_payload,
        json!({ "schemas": [schema("h1.first")] })
    );

    review_repository
        .update_schema_field(
            newer_review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, newer_review_id).await,
            0,
            "title",
            Some(rule("h1.current")),
        )
        .await
        .unwrap();
    assert_eq!(live().await, vec![schema("h1.current")]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_approved_append_review_editable_when_approval_merged_it_into_live_schemas() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let initial_review_id = review_repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1.first")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&review_repository, initial_review_id)
        .await
        .unwrap();
    let append_review_id = review_repository
        .create_schema_review(
            &listing_source_id,
            "append_schema_generation",
            &[schema("h1.appended")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();

    approve_acknowledging_coverage(&review_repository, append_review_id)
        .await
        .unwrap();

    let merged = vec![schema("h1.first"), schema("h1.appended")];
    let appended = review_repository
        .get_review(append_review_id)
        .await
        .unwrap();
    assert_eq!(
        appended.review.candidate_payload,
        json!({ "schemas": merged })
    );
    review_repository
        .update_schema_field(
            append_review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, append_review_id).await,
            1,
            "title",
            Some(rule("h1.edited")),
        )
        .await
        .unwrap();
    assert_eq!(
        schema_repository
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .unwrap()
            .product_schemas,
        vec![schema("h1.first"), schema("h1.edited")]
    );
}

/// A ListingSource whose live schema fails a product page, with a pending manual review the
/// page waits on.
struct WaitingPageFixture {
    review_repository: CrawlerReviewRepository,
    candidates: ScraperCandidateServiceImpl,
    listing_source_id: ListingSourceId,
    domain_id: CrawlerDomainId,
    review_id: CrawlerReviewId,
    url: url::Url,
}

impl WaitingPageFixture {
    async fn new(pool: &PgPool) -> Self {
        Self::on_host(pool, "example.com").await
    }

    /// The fixture for a ListingSource of its own whose shop is at `host`.
    async fn on_host(pool: &PgPool, host: &str) -> Self {
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let listing_source_id = ListingSourceId::new();
        insert_listing_source(pool, listing_source_id).await;
        let domain_id = insert_domain(pool, listing_source_id, host).await;
        let url = url::Url::parse(&format!("https://{host}/products/waiting")).unwrap();
        UrlMetadataRepositoryImpl::new(pool.clone())
            .upsert_link(
                &listing_source_id,
                &domain_id,
                &url,
                &UrlClass::ProductListing,
            )
            .await
            .unwrap();
        let initial_review_id = review_repository
            .create_schema_review(
                &listing_source_id,
                "manual_schema_required",
                &[schema("h1.first")],
                review_pages(),
                json!({}),
            )
            .await
            .unwrap();
        approve_acknowledging_coverage(&review_repository, initial_review_id)
            .await
            .unwrap();
        let review_id = review_repository
            .create_schema_review(
                &listing_source_id,
                "manual_schema_required",
                &[schema("h1.first"), schema("h1.second")],
                review_pages(),
                json!({}),
            )
            .await
            .unwrap();
        Self {
            review_repository,
            candidates: ScraperCandidateServiceImpl::new(pool.clone()),
            listing_source_id,
            domain_id,
            review_id,
            url,
        }
    }

    /// Adds another product URL of the ListingSource.
    async fn add_product_url(&self, pool: &PgPool, url: &str) -> url::Url {
        let url = url::Url::parse(url).unwrap();
        UrlMetadataRepositoryImpl::new(pool.clone())
            .upsert_link(
                &self.listing_source_id,
                &self.domain_id,
                &url,
                &UrlClass::ProductListing,
            )
            .await
            .unwrap();
        url
    }

    /// Records that the page waits on the review for the `streak`-th time and retries after
    /// `delay`.
    async fn wait(&self, streak: u32, delay: time::Duration) -> CrawlerUrlWriteOutcome {
        self.wait_on(&self.url, streak, delay).await
    }

    /// Records that the page at `url` waits on the review for the `streak`-th time and retries
    /// after `delay`.
    async fn wait_on(
        &self,
        url: &url::Url,
        streak: u32,
        delay: time::Duration,
    ) -> CrawlerUrlWriteOutcome {
        self.candidates
            .mark_waiting_on_schema_review(SchemaReviewWaitRequest {
                url_failure: FetchFailureRequest {
                    listing_source_id: self.listing_source_id,
                    url: url.clone(),
                    error_kind: "PendingSchemaReview".to_owned(),
                    error_message: "blocked pending product schema review".to_owned(),
                    status_code: None,
                    next_retry_at: OffsetDateTime::now_utc() + delay,
                    expected_last_captured_raw_input_sha256: None,
                },
                wait: SchemaReviewWait {
                    review_id: self.review_id,
                    streak,
                },
            })
            .await
            .unwrap()
    }

    async fn candidate_urls(&self) -> Vec<url::Url> {
        self.candidates
            .get_candidates(10, 10, &[])
            .await
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.url)
            .collect()
    }

    async fn persisted_wait(&self, pool: &PgPool) -> (Option<Uuid>, i32, Option<OffsetDateTime>) {
        let (_, waited_on, streak, next_retry_at) = persisted_url_wait(pool, &self.url).await;
        (waited_on, streak, next_retry_at)
    }
}

/// The error kind, review wait and retry time persisted for the crawler URL `url`.
async fn persisted_url_wait(
    pool: &PgPool,
    url: &url::Url,
) -> (Option<String>, Option<Uuid>, i32, Option<OffsetDateTime>) {
    sqlx::query_as(
        "SELECT last_error_kind, schema_review_waited_on, schema_review_wait_streak, next_retry_at \
         FROM listing_source_urls WHERE url = $1",
    )
    .bind(url.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A verdict that closes the review a page waits on.
#[derive(Clone, Copy, Debug)]
enum ClosingVerdict {
    Approve,
    Reject,
}

impl ClosingVerdict {
    async fn close(self, fixture: &WaitingPageFixture) -> Result<(), ReviewRepositoryError> {
        let repository = &fixture.review_repository;
        match self {
            Self::Approve => approve_acknowledging_coverage(repository, fixture.review_id).await,
            Self::Reject => {
                repository
                    .reject_review(fixture.review_id, ReviewActor::Human, None)
                    .await
            }
        }
    }
}

/// Closing the review a page backed off on releases the page for an immediate retry and clears
/// its wait. A page of another ListingSource keeps waiting on that source's own review.
async fn assert_closing_the_review_releases_its_backed_off_page(verdict: ClosingVerdict) {
    let pool = get_postgres_client().await;
    let fixture = WaitingPageFixture::new(&pool).await;
    let other_source = WaitingPageFixture::on_host(&pool, "other-source.example").await;
    assert_eq!(
        fixture.wait(4, time::Duration::hours(4)).await,
        CrawlerUrlWriteOutcome::Applied
    );
    assert_eq!(
        other_source.wait(4, time::Duration::hours(4)).await,
        CrawlerUrlWriteOutcome::Applied
    );
    let other_source_wait = persisted_url_wait(&pool, &other_source.url).await;
    assert!(
        fixture.candidate_urls().await.is_empty(),
        "the backed-off page must wait although cached schemas keep its source eligible"
    );

    verdict.close(&fixture).await.unwrap();

    assert_eq!(
        fixture.candidate_urls().await,
        vec![fixture.url.clone()],
        "{verdict:?} must release the backed-off page for an immediate retry"
    );
    assert_eq!(
        persisted_url_wait(&pool, &fixture.url).await,
        (None, None, 0, None)
    );
    assert_eq!(
        persisted_url_wait(&pool, &other_source.url).await,
        other_source_wait,
        "{verdict:?} must not release a page that waits on another ListingSource's review"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_pages_backed_off_on_a_schema_review_for_immediate_retry_when_approved() {
    assert_closing_the_review_releases_its_backed_off_page(ClosingVerdict::Approve).await;
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_pages_backed_off_on_a_schema_review_for_immediate_retry_when_rejected() {
    assert_closing_the_review_releases_its_backed_off_page(ClosingVerdict::Reject).await;
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_hold_back_a_released_page_when_a_scrape_that_overlapped_approval_waits() {
    let pool = get_postgres_client().await;
    let fixture = WaitingPageFixture::new(&pool).await;
    assert_eq!(
        fixture.wait(3, -time::Duration::minutes(1)).await,
        CrawlerUrlWriteOutcome::Applied
    );

    // The page's retry found the review pending; the reviewer approves it before the scrape
    // records the next, longer wait.
    approve_acknowledging_coverage(&fixture.review_repository, fixture.review_id)
        .await
        .unwrap();
    assert_eq!(
        fixture.wait(4, time::Duration::hours(4)).await,
        CrawlerUrlWriteOutcome::NoopStale
    );

    assert_eq!(fixture.candidate_urls().await, vec![fixture.url.clone()]);
    assert_eq!(fixture.persisted_wait(&pool).await, (None, 0, None));
}

/// A scrape that records its wait while the verdict commits must not hold back the page the
/// verdict releases: the verdict releases the page in the transaction that closes the review, so
/// the wait write waits for it and then finds the review closed.
async fn assert_wait_written_while_the_verdict_commits_leaves_the_page_released(
    verdict: ClosingVerdict,
) {
    let pool = get_postgres_client().await;
    let fixture = std::sync::Arc::new(WaitingPageFixture::new(&pool).await);
    assert_eq!(
        fixture.wait(3, -time::Duration::minutes(1)).await,
        CrawlerUrlWriteOutcome::Applied
    );
    // Holding the page's row stops the verdict at releasing the page, after it has closed the
    // review but before it commits.
    let mut page_lock = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM listing_source_urls WHERE url = $1 FOR UPDATE")
        .bind(fixture.url.as_str())
        .execute(&mut *page_lock)
        .await
        .unwrap();
    let closing = tokio::spawn({
        let fixture = fixture.clone();
        async move { verdict.close(&fixture).await }
    });
    wait_for_review_wait_lock_waiters(&pool, 1).await;
    assert_eq!(
        fixture
            .review_repository
            .get_review(fixture.review_id)
            .await
            .unwrap()
            .review
            .status,
        STATUS_PENDING_REVIEW,
        "{verdict:?} must release the page before it commits the closed review"
    );
    // The page's retry found the review pending and records its next, longer wait now.
    let wait = tokio::spawn({
        let fixture = fixture.clone();
        async move { fixture.wait(4, time::Duration::hours(4)).await }
    });
    wait_for_review_wait_lock_waiters(&pool, 2).await;

    page_lock.commit().await.unwrap();

    closing.await.unwrap().unwrap();
    assert_eq!(wait.await.unwrap(), CrawlerUrlWriteOutcome::NoopStale);
    assert_eq!(fixture.persisted_wait(&pool).await, (None, 0, None));
    assert_eq!(fixture.candidate_urls().await, vec![fixture.url.clone()]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_hold_back_a_released_page_when_its_wait_is_written_while_the_approval_commits()
{
    assert_wait_written_while_the_verdict_commits_leaves_the_page_released(ClosingVerdict::Approve)
        .await;
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_hold_back_a_released_page_when_its_wait_is_written_while_the_rejection_commits()
{
    assert_wait_written_while_the_verdict_commits_leaves_the_page_released(ClosingVerdict::Reject)
        .await;
}

/// Waits until `count` statements that write a page's review wait or release it are blocked
/// on a lock.
async fn wait_for_review_wait_lock_waiters(pool: &PgPool, count: i64) {
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity \
             WHERE datname = current_database() \
               AND wait_event_type = 'Lock' \
               AND query LIKE '%schema_review_wait_streak%'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{count} review wait statements never waited on a lock");
}

/// Approving a manual review whose candidate declares removed-page templates stores the
/// templates and releases the pages backed off on the review in one transaction. While the
/// stored templates do not decode, the replacement fails and the approval changes nothing: the
/// review stays pending and its pages keep their wait. Once the stored row decodes, the same
/// approval does both, keeping the template stored after the review opened.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_removed_page_templates_and_release_backed_off_pages_in_one_approval() {
    let pool = get_postgres_client().await;
    let fixture = WaitingPageFixture::new(&pool).await;
    let notice_page = fixture
        .add_product_url(&pool, "https://example.com/products/sold-chair")
        .await;
    assert_eq!(
        fixture.wait(3, time::Duration::hours(2)).await,
        CrawlerUrlWriteOutcome::Applied
    );
    assert_eq!(
        fixture
            .wait_on(&notice_page, 1, time::Duration::minutes(30))
            .await,
        CrawlerUrlWriteOutcome::Applied
    );
    let waiting_pages = [fixture.url.clone(), notice_page];
    let mut waits = Vec::new();
    for url in &waiting_pages {
        waits.push(persisted_url_wait(&pool, url).await);
    }
    let generated = RemovedPageSchema {
        selector: "#main h1".into(),
        text: Some("Page not found".to_owned()),
        regex: None,
    };
    // The stored templates are an object where the list of templates belongs.
    let undecodable = json!(generated);
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas \
         (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(fixture.listing_source_id.as_uuid())
    .bind(&undecodable)
    .execute(&pool)
    .await
    .unwrap();
    // The reviewer keeps the live schema, drops the starter schema and declares the notice
    // page removed.
    fixture
        .review_repository
        .update_candidate_payload(
            fixture.review_id,
            ReviewActor::Human,
            candidate_version(&fixture.review_repository, fixture.review_id).await,
            json!({
                "schemas": [schema("h1.first")],
                "removed_page_schemas": [notice_template("no longer available")],
            }),
        )
        .await
        .unwrap();

    let refused = approve_acknowledging_coverage(&fixture.review_repository, fixture.review_id)
        .await
        .unwrap_err();

    assert!(
        matches!(
            refused,
            ReviewRepositoryError::Database(sqlx::Error::Decode(_))
        ),
        "{refused:?}"
    );
    let review = fixture
        .review_repository
        .get_review(fixture.review_id)
        .await
        .unwrap()
        .review;
    assert_eq!(review.status, STATUS_PENDING_REVIEW);
    assert_eq!(
        stored_removed_page_templates(&pool, fixture.listing_source_id).await,
        undecodable
    );
    for (url, wait) in waiting_pages.iter().zip(&waits) {
        let kept = persisted_url_wait(&pool, url).await;
        assert_eq!(&kept, wait, "{url} must keep its wait");
        assert_eq!(kept.0.as_deref(), Some("PendingSchemaReview"));
        assert_eq!(kept.1, Some(*fixture.review_id.as_uuid()));
    }
    assert!(
        fixture.candidate_urls().await.is_empty(),
        "a failed approval must not release the backed-off pages"
    );

    sqlx::query(
        "UPDATE listing_source_removed_page_schemas SET removed_page_schema = $2 \
         WHERE listing_source_id = $1",
    )
    .bind(fixture.listing_source_id.as_uuid())
    .bind(json!([generated]))
    .execute(&pool)
    .await
    .unwrap();
    approve_acknowledging_coverage(&fixture.review_repository, fixture.review_id)
        .await
        .unwrap();

    let review = fixture
        .review_repository
        .get_review(fixture.review_id)
        .await
        .unwrap()
        .review;
    assert_eq!(review.status, STATUS_APPROVED);
    assert_eq!(
        stored_removed_page_templates(&pool, fixture.listing_source_id).await,
        json!([notice_template("no longer available"), generated])
    );
    for url in &waiting_pages {
        assert_eq!(
            persisted_url_wait(&pool, url).await,
            (None, None, 0, None),
            "approval must release {url} and clear its wait"
        );
    }
    let mut released = fixture.candidate_urls().await;
    released.sort();
    let mut expected = waiting_pages.to_vec();
    expected.sort();
    assert_eq!(released, expected);
    let live = ListingSourceProductSchemaRepositoryImpl::new(&pool)
        .find_product_schema(&fixture.listing_source_id)
        .await
        .unwrap()
        .map(|live| live.product_schemas);
    assert_eq!(live, Some(vec![schema("h1.first")]));
}

async fn stored_removed_page_templates(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT removed_page_schema FROM listing_source_removed_page_schemas \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_report_approved_review_as_stale_when_its_stored_candidate_no_longer_decodes() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = review_repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1.first")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&review_repository, review_id)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE crawler_reviews SET candidate_payload = '{\"legacy_schema\": {}}' \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let replaced = review_repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, review_id).await,
            json!({ "schemas": [schema("h1.fixed")] }),
        )
        .await;

    assert!(matches!(
        replaced,
        Err(ReviewRepositoryError::StaleApprovedReview(id)) if id == review_id
    ));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn schema_review_creation_rejects_invalid_typed_schema() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let result = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("[")],
            review_pages(),
            json!({}),
        )
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
    ));
    assert_eq!(review_count(&pool, listing_source_id).await, 0);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn approved_schema_field_edit_rejects_invalid_rule_without_mutating_live_or_audit_state() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let original_schema = schema("h1.original");
    let now = OffsetDateTime::now_utc();
    schema_repository
        .insert_product_schema(
            &listing_source_id,
            &ListingSourceProductSchema {
                listing_source_id,
                product_schemas: vec![original_schema.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "initial_schema_generation",
            schemas: std::slice::from_ref(&original_schema),
            pages: review_pages(),
            validation_summary: json!({ "auto_schema_evaluation": { "decision": "APPROVE" } }),
            status: STATUS_APPROVED,
            notes: None,
        })
        .await
        .unwrap();
    let before = review_repository.get_review(review_id).await.unwrap();

    let result = review_repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            candidate_version(&review_repository, review_id).await,
            0,
            "title",
            Some(rule("[")),
        )
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
    ));
    assert_eq!(
        schema_repository
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .unwrap()
            .product_schemas,
        vec![original_schema]
    );
    let after = review_repository.get_review(review_id).await.unwrap();
    assert_eq!(
        after.review.candidate_payload,
        before.review.candidate_payload
    );
    assert_eq!(
        after.review.validation_summary,
        before.review.validation_summary
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn schema_matrix_write_skips_stale_candidate_snapshot() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({
                "auto_schema_evaluation": { "decision": "APPROVE" },
                "manual_schema_edits": []
            }),
        )
        .await
        .unwrap();

    let pages = repository.get_review_pages(review_id).await.unwrap();
    let review_page_id = pages[0].review_page_id;
    let matrix = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| {
                    (
                        page,
                        "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_owned(),
                    )
                })
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    assert_eq!(matrix.matrix.review_id, Some(review_id));
    assert_eq!(
        matrix.matrix.candidates[0].pages[0].page_reference,
        SchemaPageReference::Persisted { review_page_id }
    );
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h2")] }),
        )
        .await
        .unwrap();
    let candidate_version: i64 =
        sqlx::query_scalar("SELECT candidate_version FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(candidate_version, 2);

    let stale_store = repository
        .store_schema_matrix_if_current(review_id, matrix)
        .await;
    assert!(matches!(
        stale_store,
        Err(ReviewRepositoryError::CandidateChangedDuringEvaluation {
            candidate_version: 2
        })
    ));

    let validation_summary: serde_json::Value =
        sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        validation_summary,
        json!({
            "auto_schema_evaluation": { "decision": "APPROVE" },
            "manual_schema_edits": []
        })
    );

    let pages = repository.get_review_pages(review_id).await.unwrap();
    let fresh_matrix = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| {
                    (
                        page,
                        "<html><body><span class=\"id\">SKU</span><h2>Title</h2><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_owned(),
                    )
                })
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, fresh_matrix)
        .await
        .unwrap();

    let validation_summary: serde_json::Value =
        sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        validation_summary["schema_matrix"]["review_id"],
        json!(review_id)
    );
    assert_eq!(
        validation_summary["schema_matrix"]["candidates"][0]["pages"][0]["page_reference"]["kind"],
        "persisted"
    );
    assert_eq!(
        validation_summary["schema_matrix"]["candidates"][0]["pages"][0]["page_reference"]["review_page_id"],
        json!(review_page_id)
    );
    assert_eq!(
        validation_summary["auto_schema_evaluation"]["decision"],
        "APPROVE"
    );
    assert_eq!(validation_summary["manual_schema_edits"], json!([]));
}

/// A validation summary as schema generation records it: the generated matrix, the summary
/// that judges it and the LLM evaluation.
fn generated_validation_summary() -> serde_json::Value {
    json!({
        "auto_schema_evaluation": { "decision": "APPROVE" },
        "schema_matrix": { "review_id": null, "candidates": [] },
        "schema_matrix_summary": { "deterministic_approval_ok": false }
    })
}

async fn generated_schema_review(
    pool: &PgPool,
    repository: &CrawlerReviewRepository,
) -> CrawlerReviewId {
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(pool, listing_source_id).await;
    repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema("h1")],
            review_pages(),
            generated_validation_summary(),
        )
        .await
        .unwrap()
}

async fn stored_validation_summary(pool: &PgPool, review_id: CrawlerReviewId) -> serde_json::Value {
    sqlx::query_scalar("SELECT validation_summary FROM crawler_reviews WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn schema_matrix_write_drops_the_generated_matrix_summary() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = generated_schema_review(&pool, &repository).await;
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| {
                    (
                        page,
                        "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_owned(),
                    )
                })
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    let stored = repository
        .store_schema_matrix_if_current(review_id, evaluated)
        .await
        .unwrap();

    assert_eq!(
        stored_validation_summary(&pool, review_id).await,
        json!({
            "auto_schema_evaluation": { "decision": "APPROVE" },
            "schema_matrix": stored.matrix,
        })
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn candidate_change_drops_the_generated_matrix_summary_with_the_cached_matrix() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = generated_schema_review(&pool, &repository).await;

    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h2")] }),
        )
        .await
        .unwrap();

    assert_eq!(
        stored_validation_summary(&pool, review_id).await,
        json!({ "auto_schema_evaluation": { "decision": "APPROVE" } })
    );
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

/// Approves the review the way a reviewer who confirmed its incomplete coverage does: with an
/// acknowledgement of the coverage the refused approval reports.
async fn approve_acknowledging_coverage(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Result<(), ReviewRepositoryError> {
    match approve(
        repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    {
        Err(ReviewRepositoryError::IncompleteCoverage { coverage, .. }) => {
            approve(
                repository,
                review_id,
                ReviewActor::Human,
                None,
                IncompleteCoverage::Acknowledged(coverage),
            )
            .await
        }
        decided => decided,
    }
}

/// A product page whose title sits in a `title_tag` element.
fn coverage_page(slug: &str, title_tag: &str) -> SchemaReviewPageInput {
    SchemaReviewPageInput {
        url: format!("https://example.com/products/{slug}"),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: format!(
            "<html><body><span class=\"id\">{slug}</span><{title_tag}>{slug}</{title_tag}><span class=\"state\">In stock</span><img class=\"product\" src=\"{slug}.jpg\"></body></html>"
        ),
    }
}

/// Evaluates the review's candidate on `pages`, the HTML its review pages serve, and stores the
/// matrix as a refresh does.
async fn store_schema_matrix(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    pages: &[SchemaReviewPageInput],
) {
    let live_pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| {
            let html = pages
                .iter()
                .find(|input| input.url == page.url)
                .map(|input| input.raw_html.clone())
                .unwrap();
            (page, html)
        })
        .collect();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            live_pages,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, evaluated)
        .await
        .unwrap();
}

async fn review_status(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) -> String {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .review
        .status
}

/// Approval reads the stored matrix and the review's pages under its row lock. A candidate that
/// leaves a review page uncovered stays pending and out of the live schemas until the approver
/// acknowledges the review's coverage.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_approve_a_candidate_that_leaves_a_review_page_uncovered_only_when_acknowledged() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let pages = vec![coverage_page("chair", "h1"), coverage_page("table", "h2")];
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            pages.clone(),
            json!({}),
        )
        .await
        .unwrap();
    store_schema_matrix(&repository, review_id, &pages).await;

    let refused = approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await;

    assert!(
        matches!(
            refused,
            Err(ReviewRepositoryError::IncompleteCoverage {
                coverage: ReviewPageCoverage {
                    covered: 1,
                    removed: 0,
                    total: 2,
                    not_evaluated: 0,
                },
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        review_status(&repository, review_id).await,
        STATUS_PENDING_REVIEW
    );
    let live_schemas = ListingSourceProductSchemaRepositoryImpl::new(&pool);
    assert!(
        live_schemas
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .is_none()
    );

    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Acknowledged(ReviewPageCoverage {
            covered: 1,
            removed: 0,
            total: 2,
            not_evaluated: 0,
        }),
    )
    .await
    .unwrap();

    assert_eq!(review_status(&repository, review_id).await, STATUS_APPROVED);
    assert!(
        live_schemas
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .is_some()
    );
}

/// Without a stored matrix no review page is evaluated; once a stored matrix covers every page,
/// approval needs no acknowledgement.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_approve_without_acknowledgement_once_the_stored_matrix_covers_every_review_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let pages = vec![coverage_page("chair", "h1"), coverage_page("table", "h1")];
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            pages.clone(),
            json!({}),
        )
        .await
        .unwrap();

    let without_matrix = approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await;
    assert!(
        matches!(
            without_matrix,
            Err(ReviewRepositoryError::IncompleteCoverage {
                coverage: ReviewPageCoverage {
                    covered: 0,
                    removed: 0,
                    total: 2,
                    not_evaluated: 2,
                },
                ..
            })
        ),
        "{without_matrix:?}"
    );

    store_schema_matrix(&repository, review_id, &pages).await;
    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    assert_eq!(review_status(&repository, review_id).await, STATUS_APPROVED);
}

/// The live matrix compares each page's extraction with the identity the page's listing
/// was captured under, read from the crawler URL of the review's ListingSource that names
/// the page, however the two URLs spell it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn live_schema_matrix_should_report_an_identity_that_differs_from_the_captured_one() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "example.com").await;
    let captured_url = "https://example.com/products/1";
    let uncaptured_url = "https://example.com/products/2";
    // A reviewer page is stored as fetched: the shop redirects to the trailing slash that
    // the crawler URL's normalization drops.
    let respelled_crawler_url = "https://example.com/products/3";
    let respelled_page_url = "https://example.com/products/3/";
    for (url, identity) in [
        (captured_url, Some("53036")),
        (uncaptured_url, None),
        (respelled_crawler_url, Some("53037")),
    ] {
        sqlx::query(
            "INSERT INTO listing_source_urls \
             (listing_source_id, domain_id, url, url_class, raw_source_record_key, last_source_listing_id) \
             VALUES ($1, $2, $3, 'product', $3, $4)",
        )
        .bind(listing_source_id.as_uuid())
        .bind(domain_id.as_uuid())
        .bind(url)
        .bind(identity)
        .execute(&pool)
        .await
        .unwrap();
    }
    let html = "<html><body><span class=\"id\">product-53036</span><h1>Chair</h1><span class=\"state\">Sold</span><img class=\"product\" src=\"a.jpg\"></body></html>";
    let pages = [captured_url, uncaptured_url, respelled_page_url]
        .into_iter()
        .map(|url| SchemaReviewPageInput {
            url: url.to_owned(),
            role: PAGE_ROLE_PRIMARY.to_owned(),
            raw_html: html.to_owned(),
        })
        .collect();
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            pages,
            json!({}),
        )
        .await
        .unwrap();

    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| (page, html.to_owned()))
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    let findings: Vec<(String, serde_json::Value)> = evaluated.matrix.candidates[0]
        .pages
        .iter()
        .map(|page| {
            (
                page.url.clone(),
                serde_json::to_value(&page.identity_agreement_findings).unwrap(),
            )
        })
        .collect();
    assert!(findings.contains(&(
        captured_url.to_owned(),
        json!([{
            "field": "source_listing_id",
            "check": "DIFFERS_FROM_CAPTURED_IDENTITY",
            "values": ["53036"]
        }])
    )));
    assert!(findings.contains(&(uncaptured_url.to_owned(), json!([]))));
    assert!(findings.contains(&(
        respelled_page_url.to_owned(),
        json!([{
            "field": "source_listing_id",
            "check": "DIFFERS_FROM_CAPTURED_IDENTITY",
            "values": ["53037"]
        }])
    )));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn concurrent_schema_reviews_return_same_pending_review_without_duplicate_pages() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;

    let schema = schema("h1");
    let first_repo = review_repository.clone();
    let first_schema = schema.clone();
    let second_repo = review_repository.clone();
    let second_schema = schema.clone();

    let (first, second) = tokio::join!(
        async move {
            first_repo
                .create_schema_review(
                    &listing_source_id,
                    "initial_schema_generation",
                    &[first_schema],
                    review_pages(),
                    json!({ "source": "first" }),
                )
                .await
        },
        async move {
            second_repo
                .create_schema_review(
                    &listing_source_id,
                    "initial_schema_generation",
                    &[second_schema],
                    review_pages(),
                    json!({ "source": "second" }),
                )
                .await
        }
    );

    let first_id = first.unwrap();
    let second_id = second.unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(
        pending_review_count(&pool, listing_source_id, "PRODUCT_SCHEMA").await,
        1
    );
    assert_eq!(review_page_count(&pool, first_id).await, 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn concurrent_url_pattern_reviews_return_same_pending_review_without_duplicate_urls() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "review-pattern.example.com").await;

    let pattern = Regex::new("/product/").unwrap();
    let urls = vec![
        "https://example.com/product/1".to_string(),
        "https://example.com/about".to_string(),
    ];
    let first_repo = review_repository.clone();
    let first_urls = urls.clone();
    let second_repo = review_repository.clone();
    let second_urls = urls.clone();

    let (first, second) = tokio::join!(
        async move {
            first_repo
                .create_url_pattern_review(
                    &listing_source_id,
                    &domain_id,
                    "url_pattern_generation",
                    UrlPatternDecision::inferred(Some(&pattern)),
                    &first_urls,
                    None,
                )
                .await
        },
        async move {
            let pattern = Regex::new("/product/").unwrap();
            second_repo
                .create_url_pattern_review(
                    &listing_source_id,
                    &domain_id,
                    "url_pattern_generation",
                    UrlPatternDecision::inferred(Some(&pattern)),
                    &second_urls,
                    None,
                )
                .await
        }
    );

    let first_id = first.unwrap();
    let second_id = second.unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(
        pending_review_count(&pool, listing_source_id, "URL_PATTERN").await,
        1
    );
    assert_eq!(review_url_count(&pool, first_id).await, 2);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn invalid_edited_url_pattern_is_rejected_without_changing_live_pattern() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "typed-pattern.example.com").await;
    let pattern = Regex::new("/products/").unwrap();
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            UrlPatternDecision::inferred(Some(&pattern)),
            &["https://typed-pattern.example.com/products/1".to_owned()],
            None,
        )
        .await
        .unwrap();

    let result = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "[".to_owned(),
                },
                current_pattern: None,
                sample_labels: None,
            }),
        )
        .await;

    assert!(matches!(
        result,
        Err(ReviewRepositoryError::InvalidUrlPatternCandidate)
    ));
    let live_pattern: Option<String> =
        sqlx::query_scalar("SELECT url_pattern FROM listing_source_domains WHERE domain_id = $1")
            .bind(domain_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(live_pattern.is_none());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_roll_back_pattern_approval_when_status_update_fails() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "atomic-pattern.example.com").await;
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
            &["https://atomic-pattern.example.com/new/1".to_owned()],
            Some(&Regex::new("/old/").unwrap()),
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION fail_review_approval() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF NEW.status = 'APPROVED' THEN RAISE EXCEPTION 'injected approval failure'; END IF; \
           RETURN NEW; \
         END; $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The backing UUID comes from a generated typed ID, so this audited DDL is safe.
    sqlx::query(AssertSqlSafe(format!(
        "CREATE TRIGGER fail_review_approval BEFORE UPDATE ON crawler_reviews \
         FOR EACH ROW WHEN (NEW.review_id = '{}'::uuid) \
         EXECUTE FUNCTION fail_review_approval()",
        review_id.as_uuid()
    )))
    .execute(&pool)
    .await
    .unwrap();

    let result = approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await;

    assert!(result.is_err());
    let pattern: (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pattern, (Some("/old/".to_owned()), "MATCHED".to_owned()));
    let status: String =
        sqlx::query_scalar("SELECT status FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, STATUS_PENDING_REVIEW);
    sqlx::query("DROP TRIGGER fail_review_approval ON crawler_reviews")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION fail_review_approval()")
        .execute(&pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_allow_only_one_concurrent_review_approval() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id =
        insert_domain(&pool, listing_source_id, "concurrent-approval.example.com").await;
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "url_pattern_generation",
            UrlPatternDecision::inferred(Some(&Regex::new("/product/").unwrap())),
            &["https://concurrent-approval.example.com/product/1".to_owned()],
            None,
        )
        .await
        .unwrap();
    let first = repository.clone();
    let second = repository.clone();

    let (first, second) = tokio::join!(
        async move {
            approve(
                &first,
                review_id,
                ReviewActor::Human,
                None,
                IncompleteCoverage::Refuse,
            )
            .await
        },
        async move {
            approve(
                &second,
                review_id,
                ReviewActor::Human,
                None,
                IncompleteCoverage::Refuse,
            )
            .await
        }
    );

    assert!(first.is_ok() ^ second.is_ok());
    assert!(matches!(
        first.err().or(second.err()),
        Some(ReviewRepositoryError::NotPending(id)) if id == review_id
    ));
    let status: String =
        sqlx::query_scalar("SELECT status FROM crawler_reviews WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, STATUS_APPROVED);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn approved_no_pattern_clears_stale_live_pattern_and_sets_no_pattern_state() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "no-pattern.example.com").await;
    sqlx::query(
        "UPDATE listing_source_domains SET url_pattern = '/stale/', url_pattern_state = 'MATCHED' WHERE domain_id = $1",
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
            UrlPatternDecision::NoPattern,
            &["https://no-pattern.example.com/about".to_owned()],
            Some(&Regex::new("/stale/").unwrap()),
        )
        .await
        .unwrap();

    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    let row: (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, "NO_PATTERN".to_owned()));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_approve_manual_url_pattern_review_only_after_operator_decides() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "manual-pattern.example.com").await;
    let review_id = repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            &[
                "https://manual-pattern.example.com/item/1".to_owned(),
                "https://manual-pattern.example.com/about".to_owned(),
            ],
            None,
        )
        .await
        .unwrap();

    let undecided_approval = approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await;
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "/item/".to_owned(),
                },
                current_pattern: None,
                sample_labels: None,
            }),
        )
        .await
        .unwrap();
    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    assert!(matches!(
        undecided_approval,
        Err(ReviewRepositoryError::InvalidUrlPatternCandidate)
    ));
    let (pattern, state): (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pattern.as_deref(), Some("/item/"));
    assert_eq!(state, "MATCHED");
}

/// Marks the domain as a crawl left it when it stopped at a pending URL-pattern review.
async fn mark_domain_waiting_for_url_pattern_review(pool: &PgPool, domain_id: CrawlerDomainId) {
    sqlx::query(
        "UPDATE listing_source_domains \
         SET last_crawl_outcome = $2, next_crawl_at = NOW() + INTERVAL '7 days', \
             crawl_failure_count = 1, last_crawl_error_kind = 'RateLimited' \
         WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .bind(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
    .execute(pool)
    .await
    .unwrap();
}

async fn domain_crawl_schedule(
    pool: &PgPool,
    domain_id: CrawlerDomainId,
) -> (Option<OffsetDateTime>, i32, Option<String>) {
    sqlx::query_as(
        "SELECT next_crawl_at, crawl_failure_count, last_crawl_error_kind \
         FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn stored_url_classes(pool: &PgPool, domain_id: CrawlerDomainId) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT url, url_class FROM listing_source_urls WHERE domain_id = $1 ORDER BY url",
    )
    .bind(domain_id.as_uuid())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn undecided_url_pattern_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
    domain_id: CrawlerDomainId,
    sample: &[String],
) -> CrawlerReviewId {
    repository
        .create_url_pattern_review(
            &listing_source_id,
            &domain_id,
            "manual_url_pattern_required",
            UrlPatternDecision::Undecided,
            sample,
            None,
        )
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_listing_source_with_a_domain_waiting_for_an_operator_first() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let reviewed_source = ListingSourceId::new();
    insert_listing_source(&pool, reviewed_source).await;
    let reviewed_domain = insert_domain(&pool, reviewed_source, "reviewed.example.com").await;
    undecided_url_pattern_review(
        &repository,
        reviewed_source,
        reviewed_domain,
        &["https://reviewed.example.com/item/1".to_owned()],
    )
    .await;
    let stopped_source = ListingSourceId::new();
    insert_listing_source(&pool, stopped_source).await;
    let stopped_domain = insert_domain(&pool, stopped_source, "stopped.example.com").await;
    SpiderCandidateServiceImpl::new(pool.clone())
        .mark_crawl_needs_operator(&stopped_domain, "RobotsBlocked", 5)
        .await
        .unwrap();

    let overview = repository.list_listing_sources(1).await.unwrap();

    assert_eq!(
        overview
            .iter()
            .map(|source| source.listing_source_id)
            .collect::<Vec<_>>(),
        vec![stopped_source],
        "a domain waiting for an operator outranks a pending review within the limit"
    );
    let domains = &overview[0].domains;
    assert_eq!(domains.len(), 1);
    assert_eq!(domains[0].domain_id, stopped_domain);
    assert_eq!(
        domains[0].last_crawl_outcome.as_deref(),
        Some(NEEDS_OPERATOR_CRAWL_OUTCOME)
    );
    assert_eq!(
        domains[0].last_crawl_error_kind.as_deref(),
        Some("RobotsBlocked")
    );
    assert_eq!(domains[0].crawl_failure_count, 5);
    assert_eq!(domains[0].next_crawl_at, None);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_list_pending_reviews_of_disabled_listing_sources_after_those_of_enabled_ones() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let enabled_source = ListingSourceId::new();
    insert_listing_source(&pool, enabled_source).await;
    let disabled_source = ListingSourceId::new();
    insert_listing_source(&pool, disabled_source).await;
    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(disabled_source.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let enabled_pending = pending_schema_review(&repository, &enabled_source).await;
    let disabled_pending = pending_schema_review(&repository, &disabled_source).await;
    let closed = repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &enabled_source,
            reason: "initial_schema_generation",
            schemas: &[schema("h1")],
            pages: review_pages(),
            validation_summary: json!({}),
            status: STATUS_APPROVED,
            notes: None,
        })
        .await
        .unwrap();

    let queue = repository.list_reviews(10).await.unwrap();

    assert_eq!(
        queue
            .iter()
            .map(|review| (review.review_id, review.listing_source_crawl_enabled))
            .collect::<Vec<_>>(),
        vec![
            (enabled_pending, true),
            (disabled_pending, false),
            (closed, true)
        ],
        "a newer pending review of a disabled ListingSource follows the older one of an enabled ListingSource"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_classify_stored_urls_and_release_waiting_domain_when_url_pattern_is_approved() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let url_repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "waiting-pattern.example.com").await;
    let urls = [
        "https://waiting-pattern.example.com/item/1",
        "https://waiting-pattern.example.com/item/2",
        "https://waiting-pattern.example.com/item/sold",
        "https://waiting-pattern.example.com/about",
    ]
    .map(|url| url::Url::parse(url).unwrap());
    // Stored by the waiting crawl without a pattern, so nothing is a product yet.
    url_repository
        .upsert_links_batch(&listing_source_id, &domain_id, &urls, &[UrlClass::Other; 4])
        .await
        .unwrap();
    url_repository
        .set_disposition(
            &listing_source_id,
            &urls[2],
            CrawlerDisposition::DormantSold,
        )
        .await
        .unwrap();
    mark_domain_waiting_for_url_pattern_review(&pool, domain_id).await;
    let review_id = undecided_url_pattern_review(
        &repository,
        listing_source_id,
        domain_id,
        &urls.iter().map(ToString::to_string).collect::<Vec<_>>(),
    )
    .await;
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "/item/".to_owned(),
                },
                current_pattern: None,
                sample_labels: None,
            }),
        )
        .await
        .unwrap();

    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    assert_eq!(
        stored_url_classes(&pool, domain_id).await,
        vec![
            (urls[3].to_string(), "info".to_owned()),
            (urls[0].to_string(), "product".to_owned()),
            (urls[1].to_string(), "product".to_owned()),
            (urls[2].to_string(), "other".to_owned()),
        ],
        "approval classifies the stored active URLs as the spider would; a sold URL keeps its class"
    );
    let (next_crawl_at, failure_count, error_kind) = domain_crawl_schedule(&pool, domain_id).await;
    assert_eq!(
        next_crawl_at, None,
        "the decided review releases the domain"
    );
    assert_eq!(
        (failure_count, error_kind.as_deref()),
        (1, Some("RateLimited")),
        "earlier failure state is left as it was"
    );
}

/// In `agent` mode the review rail classifies as the spider does there, by the pattern alone: the
/// review's URL sample carries no keyword class, approval makes the URLs the pattern takes
/// product URLs and leaves every other one unclassified, on a shop whose domain the keywords read
/// as a category, and restoring the decision it replaced leaves every URL unclassified again.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_classify_the_sample_and_stored_urls_by_the_pattern_alone_in_agent_mode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone())
        .with_url_classifier(UrlClassifier::for_authoring(CrawlerAuthoringMode::Agent));
    let url_repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "antique-shop.example.com").await;
    let urls = [
        "https://antique-shop.example.com/item/1",
        "https://antique-shop.example.com/item/2",
        "https://antique-shop.example.com/about",
    ]
    .map(|url| url::Url::parse(url).unwrap());
    url_repository
        .upsert_links_batch(&listing_source_id, &domain_id, &urls, &[UrlClass::Other; 3])
        .await
        .unwrap();
    let review_id = undecided_url_pattern_review(
        &repository,
        listing_source_id,
        domain_id,
        &urls.iter().map(ToString::to_string).collect::<Vec<_>>(),
    )
    .await;
    let sample_classes: Vec<Option<String>> = repository
        .get_review(review_id)
        .await
        .unwrap()
        .urls
        .into_iter()
        .map(|url| url.candidate_class)
        .collect();
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "decision": { "type": "PATTERN", "value": "/item/" }, "current_pattern": null }),
        )
        .await
        .unwrap();

    approve(
        &repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap();

    let approved_classes = stored_url_classes(&pool, domain_id).await;
    repository
        .restore_approval(review_id, ReviewActor::Human, None)
        .await
        .unwrap();

    assert_eq!(sample_classes, vec![Some("other".to_owned()); 3]);
    assert_eq!(
        approved_classes,
        vec![
            (urls[2].to_string(), "other".to_owned()),
            (urls[0].to_string(), "product".to_owned()),
            (urls[1].to_string(), "product".to_owned()),
        ]
    );
    assert_eq!(
        stored_url_classes(&pool, domain_id).await,
        vec![
            (urls[2].to_string(), "other".to_owned()),
            (urls[0].to_string(), "other".to_owned()),
            (urls[1].to_string(), "other".to_owned()),
        ],
        "restoring leaves every URL unclassified, where the keywords read the shop's domain as a category"
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_waiting_domain_without_changing_pattern_when_url_pattern_is_rejected() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let domain_id = insert_domain(&pool, listing_source_id, "rejected-pattern.example.com").await;
    mark_domain_waiting_for_url_pattern_review(&pool, domain_id).await;
    let review_id = undecided_url_pattern_review(
        &repository,
        listing_source_id,
        domain_id,
        &["https://rejected-pattern.example.com/item/1".to_owned()],
    )
    .await;

    repository
        .reject_review(review_id, ReviewActor::Human, Some("not a shop"))
        .await
        .unwrap();

    let (next_crawl_at, ..) = domain_crawl_schedule(&pool, domain_id).await;
    assert_eq!(next_crawl_at, None);
    let (pattern, state): (Option<String>, String) = sqlx::query_as(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains WHERE domain_id = $1",
    )
    .bind(domain_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((pattern, state.as_str()), (None, "UNKNOWN"));
    assert!(matches!(
        repository
            .reject_review(review_id, ReviewActor::Human, None)
            .await,
        Err(ReviewRepositoryError::NotPending(_))
    ));
}

async fn pending_schema_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
) -> CrawlerReviewId {
    repository
        .create_schema_review(
            listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap()
}

/// Two writers that read the same candidate version, such as a reviewer and the review agent,
/// edit at once: the review's row lock applies one edit and refuses the other with the version
/// it would overwrite, instead of letting it silently replace the first. The test holds the
/// review row until both edits wait on it, so both are in flight before either applies.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_the_second_of_two_concurrent_edits_based_on_the_same_candidate_version() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let read_version = candidate_version(&repository, review_id).await;
    let mut review_lock = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM crawler_reviews WHERE review_id = $1 FOR UPDATE")
        .bind(review_id.as_uuid())
        .execute(&mut *review_lock)
        .await
        .unwrap();

    let (candidate_edit, field_edit, ()) = tokio::join!(
        repository.update_candidate_payload(
            review_id,
            ReviewActor::Human,
            read_version,
            json!({ "schemas": [schema("h1.candidate-edit")] }),
        ),
        repository.update_schema_field(
            review_id,
            ReviewActor::Human,
            read_version,
            0,
            "title",
            Some(rule("h1.field-edit")),
        ),
        async {
            wait_for_review_lock_waiters(&pool, 2).await;
            review_lock.commit().await.unwrap();
        },
    );

    let (applied, applied_title, refused) = match (candidate_edit, field_edit) {
        (Ok(applied), Err(refused)) => (applied, "h1.candidate-edit", refused),
        (Err(refused), Ok(applied)) => (applied, "h1.field-edit", refused),
        outcome => panic!("exactly one of the edits must apply: {outcome:?}"),
    };
    assert_eq!(applied, read_version + 1);
    assert!(
        matches!(
            refused,
            ReviewRepositoryError::StaleCandidateVersion { review_id: refused_id, expected, current }
                if refused_id == review_id && expected == read_version && current == applied
        ),
        "{refused:?}"
    );
    let review = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(review.candidate_version, applied);
    assert_eq!(
        review.candidate_payload["schemas"][0]["title"]["selector"],
        applied_title
    );
}

/// Waits until `count` statements on reviews are blocked on a lock.
async fn wait_for_review_lock_waiters(pool: &PgPool, count: i64) {
    for _ in 0..500 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pg_stat_activity \
             WHERE datname = current_database() \
               AND wait_event_type = 'Lock' \
               AND query LIKE '%crawler_reviews%'",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if waiting >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{count} review statements never waited on a lock");
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_repair_request_without_a_note_or_once_the_review_is_closed() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    for note in [None, Some(" \n ")] {
        assert!(matches!(
            repository
                .request_repair(review_id, ReviewActor::Human, note)
                .await,
            Err(ReviewRepositoryError::MissingRepairNote)
        ));
    }
    let review = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(review.repair_request, None);

    repository
        .reject_review(review_id, ReviewActor::Human, None)
        .await
        .unwrap();
    assert!(matches!(
        repository.request_repair(review_id, ReviewActor::Human, Some("fix the title")).await,
        Err(ReviewRepositoryError::NotPending(id)) if id == review_id
    ));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_repair_requested_review_pending_editable_and_approvable() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    repository
        .request_repair(
            review_id,
            ReviewActor::Human,
            Some("title misses sold pages"),
        )
        .await
        .unwrap();
    repository
        .request_repair(
            review_id,
            ReviewActor::Human,
            Some("title and state miss sold pages"),
        )
        .await
        .unwrap();
    let repaired = schema("h1.product-title");
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [repaired.clone()] }),
        )
        .await
        .unwrap();
    let waiting = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(waiting.status, STATUS_PENDING_REVIEW);
    assert_eq!(
        waiting
            .repair_request
            .as_ref()
            .map(|request| request.note.as_str()),
        Some("title and state miss sold pages")
    );
    assert_eq!(
        repository
            .latest_pending_review_id(&listing_source_id, "PRODUCT_SCHEMA")
            .await
            .unwrap(),
        Some(review_id)
    );
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    let approved = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(approved.status, STATUS_APPROVED);
    assert_eq!(approved.repair_request, waiting.repair_request);
    let live = ListingSourceProductSchemaRepositoryImpl::new(&pool)
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.product_schemas, vec![repaired]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_store_a_repair_request_without_both_its_time_and_note() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    for half_recorded in [
        "UPDATE crawler_reviews SET repair_requested = NOW() WHERE review_id = $1",
        "UPDATE crawler_reviews SET repair_note = 'fix the title' WHERE review_id = $1",
        "UPDATE crawler_reviews SET repair_requested = NOW(), repair_note = ' ' WHERE review_id = $1",
    ] {
        let error = sqlx::query(half_recorded)
            .bind(review_id.as_uuid())
            .execute(&pool)
            .await
            .expect_err(half_recorded);
        assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            Some("crawler_reviews_repair_request_shape_check"),
            "{half_recorded}"
        );
    }
}

fn notice_template(text: &str) -> serde_json::Value {
    json!({ "selector": "p.notice", "text": text })
}

fn notice_schema(text: &str) -> RemovedPageSchema {
    RemovedPageSchema {
        selector: "p.notice".into(),
        text: Some(text.to_owned()),
        regex: None,
    }
}

async fn store_removed_page_templates(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    removed_page_schemas: Vec<RemovedPageSchema>,
) {
    let repository = RemovedPageSchemaRepositoryImpl::new(pool);
    if repository
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap()
        .is_some()
    {
        repository
            .update_removed_page_schema(&listing_source_id, &removed_page_schemas)
            .await
            .unwrap();
        return;
    }
    let now = OffsetDateTime::now_utc();
    repository
        .insert_removed_page_schema(
            &listing_source_id,
            &ListingSourceRemovedPageSchema {
                listing_source_id,
                removed_page_schemas,
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
}

async fn stored_removed_page_schemas(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
) -> Option<Vec<RemovedPageSchema>> {
    RemovedPageSchemaRepositoryImpl::new(pool)
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|row| row.removed_page_schemas)
}

/// A review opens with the stored removed-page templates in its candidate. The reviewer edits
/// one and retires another, approval replaces the stored list with the reviewed one, the review
/// keeps it, and an approved review cannot change it afterwards.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_replace_stored_removed_page_templates_with_the_reviewed_list_on_approval() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    store_removed_page_templates(
        &pool,
        listing_source_id,
        vec![notice_schema("sold"), notice_schema("withdrawn")],
    )
    .await;

    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    let opened = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        opened.review.candidate_payload["removed_page_schemas"],
        json!([notice_template("sold"), notice_template("withdrawn")])
    );
    let candidate = json!({
        "schemas": [schema("h1")],
        "removed_page_schemas": [notice_template("sold out")],
    });
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            candidate.clone(),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(vec![notice_schema("sold out")])
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.candidate_payload, candidate);
    let refused = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [notice_template("no longer available")],
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(id) if id == review_id
    ));
}

/// An explicitly empty reviewed list retires every stored template.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_retire_every_stored_removed_page_template_when_the_reviewed_list_is_empty() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    store_removed_page_templates(&pool, listing_source_id, vec![notice_schema("sold")]).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h1")], "removed_page_schemas": [] }),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(Vec::new())
    );
}

/// A candidate posted without `removed_page_schemas`, as a reviewer who edits only the schemas
/// sends it, keeps the templates the candidate holds, so approval keeps the stored ones.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_stored_templates_through_approval_when_a_candidate_leaves_them_out() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let stored = vec![notice_schema("sold"), notice_schema("withdrawn")];
    store_removed_page_templates(&pool, listing_source_id, stored.clone()).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h1.edited")] }),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(stored.clone())
    );
    let live = ListingSourceProductSchemaRepositoryImpl::new(&pool)
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|live| live.product_schemas);
    assert_eq!(live, Some(vec![schema("h1.edited")]));
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.candidate_payload["removed_page_schemas"],
        json!(stored)
    );
}

/// An approved review's templates are read-only, but a candidate that leaves them out does not
/// change them: the schema edit goes live and the templates stay as they are.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_accept_a_schema_edit_of_an_approved_review_that_leaves_its_templates_out() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    store_removed_page_templates(&pool, listing_source_id, vec![notice_schema("sold")]).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h1.edited")] }),
        )
        .await
        .unwrap();

    let live = ListingSourceProductSchemaRepositoryImpl::new(&pool)
        .find_product_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|live| live.product_schemas);
    assert_eq!(live, Some(vec![schema("h1.edited")]));
    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(vec![notice_schema("sold")])
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.candidate_payload,
        json!({
            "schemas": [schema("h1.edited")],
            "removed_page_schemas": [notice_template("sold")],
        })
    );
}

/// A stored template that today's validation rejects refuses approval only: a field edit that
/// leaves the templates alone still saves, and the template still blocks the approval.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_accept_a_schema_field_edit_while_a_stored_template_fails_validation() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let rejected = RemovedPageSchema {
        selector: "p.notice".into(),
        text: None,
        regex: Some(".*".to_owned()),
    };
    store_removed_page_templates(&pool, listing_source_id, vec![rejected.clone()]).await;
    // A generated append review: a field edit merges the live schemas into its candidate first.
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "append_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();

    repository
        .update_schema_field(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            0,
            "price",
            Some(rule("span.price")),
        )
        .await
        .unwrap();
    let refused = approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap_err();

    let review = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(
        review.candidate_payload["schemas"][0]["price"],
        json!(rule("span.price"))
    );
    assert_eq!(
        review.candidate_payload["removed_page_schemas"],
        json!([rejected])
    );
    assert!(
        matches!(
            refused,
            ReviewRepositoryError::InvalidRemovedPageTemplate { index: 0, .. }
        ),
        "{refused:?}"
    );
    assert_eq!(review.status, STATUS_PENDING_REVIEW);
}

/// Templates another writer stores or retires while a review is pending survive its approval:
/// the reviewed list is rebased onto them instead of reverting them.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_template_changes_made_while_the_review_was_pending() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    store_removed_page_templates(
        &pool,
        listing_source_id,
        vec![notice_schema("sold"), notice_schema("withdrawn")],
    )
    .await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [notice_template("withdrawn"), notice_template("reserved")],
            }),
        )
        .await
        .unwrap();
    // A generated template is stored and "withdrawn" retired after the review opened.
    store_removed_page_templates(
        &pool,
        listing_source_id,
        vec![notice_schema("sold"), notice_schema("page not found")],
    )
    .await;

    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();

    let stored = vec![notice_schema("reserved"), notice_schema("page not found")];
    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(stored.clone())
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.candidate_payload["removed_page_schemas"],
        json!(stored)
    );
}

/// A later review that changed the stored templates cannot be overwritten through an older
/// approved review: its templates stay read-only and its schema edits are stale.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_let_an_older_approved_review_overwrite_a_newer_template_list() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let older = pending_schema_review(&repository, &listing_source_id).await;
    let older_candidate = json!({
        "schemas": [schema("h1")],
        "removed_page_schemas": [notice_template("sold")],
    });
    repository
        .update_candidate_payload(
            older,
            ReviewActor::Human,
            candidate_version(&repository, older).await,
            older_candidate.clone(),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, older)
        .await
        .unwrap();
    let newer = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .update_candidate_payload(
            newer,
            ReviewActor::Human,
            candidate_version(&repository, newer).await,
            json!({
                "schemas": [schema("h1.newer")],
                "removed_page_schemas": [notice_template("reserved")],
            }),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, newer)
        .await
        .unwrap();

    let template_edit = repository
        .update_candidate_payload(
            older,
            ReviewActor::Human,
            candidate_version(&repository, older).await,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [notice_template("withdrawn")],
            }),
        )
        .await
        .unwrap_err();
    let schema_edit = repository
        .update_candidate_payload(
            older,
            ReviewActor::Human,
            candidate_version(&repository, older).await,
            json!({
                "schemas": [schema("h1.edited")],
                "removed_page_schemas": [notice_template("sold")],
            }),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        template_edit,
        ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(id) if id == older
    ));
    assert!(matches!(
        schema_edit,
        ReviewRepositoryError::StaleApprovedReview(id) if id == older
    ));
    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(vec![notice_schema("reserved")])
    );
}

/// The schema matrix evaluates the stored templates the candidate starts from, so a stored
/// template that matches a product page shows as over-broad before approval.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_show_a_stored_template_that_matches_a_product_page_in_the_schema_matrix() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    store_removed_page_templates(&pool, listing_source_id, vec![notice_schema("sold")]).await;
    let html = "<html><body><span class=\"id\">SKU</span><h1>Chair</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"><aside><p class=\"notice\">Sold</p></aside></body></html>";
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_owned(),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                raw_html: html.to_owned(),
            }],
            json!({}),
        )
        .await
        .unwrap();
    let pages = repository.get_review_pages(review_id).await.unwrap();

    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            pages
                .into_iter()
                .map(|page| (page, html.to_owned()))
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    assert_eq!(
        serde_json::to_value(&evaluated.matrix.removed_page_matches).unwrap(),
        json!([{
            "page_reference": evaluated.matrix.removed_page_matches[0].page_reference,
            "url": "https://example.com/products/1",
            "role": PAGE_ROLE_PRIMARY,
            "outcome": "PRODUCT_STILL_EXTRACTS",
            "templates": [{ "template_index": 0, "notices": ["sold"] }],
        }])
    );
}

/// Opening a review fails while the stored removed-page templates do not decode, instead of
/// starting the candidate from a list that approval would then overwrite.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_open_a_schema_review_while_the_stored_templates_do_not_decode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas \
         (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!({ "selector": "#main h1", "text": "Page not found" }))
    .execute(&pool)
    .await
    .unwrap();

    let refused = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            refused,
            ReviewRepositoryError::Database(sqlx::Error::Decode(_))
        ),
        "{refused:?}"
    );
    assert_eq!(review_count(&pool, listing_source_id).await, 0);
}

/// A stored template that today's validation rejects, such as a regex stored before regexes
/// that match any text were refused, opens in the candidate like any stored one. The review and
/// its matrix still load, so the reviewer sees the template, and approval refuses it until the
/// reviewer edits or retires it.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_a_stored_template_that_validation_now_rejects_editable_until_approval() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let rejected = json!({ "selector": "p.notice", "regex": ".*" });
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas \
         (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(json!([rejected]))
    .execute(&pool)
    .await
    .unwrap();
    // A generated append review: reading it merges the live schemas into its candidate.
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "append_schema_generation",
            &[schema("h1")],
            review_pages(),
            json!({}),
        )
        .await
        .unwrap();

    let opened = repository.get_review(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            opened
                .pages
                .iter()
                .cloned()
                .zip(review_pages().into_iter().map(|page| page.raw_html))
                .collect(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    let refused = approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap_err();

    assert_eq!(
        opened.review.candidate_payload["removed_page_schemas"],
        json!([rejected])
    );
    assert!(evaluated.matrix.removed_page_matches.is_empty());
    assert!(
        matches!(
            refused,
            ReviewRepositoryError::InvalidRemovedPageTemplate { index: 0, .. }
        ),
        "{refused:?}"
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
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({ "schemas": [schema("h1")], "removed_page_schemas": [] }),
        )
        .await
        .unwrap();
    approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap();
    assert_eq!(
        stored_removed_page_schemas(&pool, listing_source_id).await,
        Some(Vec::new())
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_pending_candidate_whose_removed_page_regex_matches_any_text() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    let refused = repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [{ "selector": "body", "regex": ".*" }],
            }),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        refused,
        ReviewRepositoryError::InvalidRemovedPageTemplate { index: 0, .. }
    ));
    let detail = repository.get_review(review_id).await.unwrap();
    assert!(
        detail
            .review
            .candidate_payload
            .get("removed_page_schemas")
            .is_none()
    );
}

/// A stored removed-page row that no longer decodes fails the approval instead of being
/// overwritten with the reviewed templates: the review stays pending and nothing goes live.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_the_review_pending_when_the_stored_removed_page_templates_do_not_decode() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let corrupt = json!({ "selector": "#main h1", "text": "Page not found" });
    sqlx::query(
        "INSERT INTO listing_source_removed_page_schemas \
         (listing_source_id, removed_page_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(&corrupt)
    .execute(&pool)
    .await
    .unwrap();
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [notice_template("sold")],
            }),
        )
        .await
        .unwrap();

    let refused = approve_acknowledging_coverage(&repository, review_id)
        .await
        .unwrap_err();

    assert!(
        matches!(
            refused,
            ReviewRepositoryError::Database(sqlx::Error::Decode(_))
        ),
        "{refused:?}"
    );
    let review = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(review.status, STATUS_PENDING_REVIEW);
    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT removed_page_schema FROM listing_source_removed_page_schemas \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, corrupt);
    assert!(
        ListingSourceProductSchemaRepositoryImpl::new(&pool)
            .find_product_schema(&listing_source_id)
            .await
            .unwrap()
            .is_none()
    );
}
