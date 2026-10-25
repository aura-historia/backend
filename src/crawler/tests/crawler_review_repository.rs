use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, STATUS_APPROVED,
    STATUS_PENDING_REVIEW, SchemaPageReference, SchemaReviewPageInput,
};
use crawler::review::model::{UrlPatternDecision, UrlPatternReviewCandidate};
use crawler::review::repository::{
    CrawlerReviewRepository, ReviewRepositoryError, SchemaReviewWithStatusInput,
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
use crawler::spider::classification::url_metadata::{CrawlerUrlWriteOutcome, UrlClass};
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
             ('crawler_review_urls', 'review_url_id') \
           )",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(counts, (4, 0));
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
        .update_candidate_payload(review_id, json!({ "schemas": [updated_schema.clone()] }))
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
    review_repository
        .approve_review(older_review_id, None)
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
    review_repository
        .approve_review(newer_review_id, None)
        .await
        .unwrap();

    let candidate_edit = review_repository
        .update_candidate_payload(
            older_review_id,
            json!({ "schemas": [schema("h1.stale-edit")] }),
        )
        .await;
    let field_edit = review_repository
        .update_schema_field(older_review_id, 0, "title", Some(rule("h1.stale-field")))
        .await;

    assert!(matches!(
        candidate_edit,
        Err(ReviewRepositoryError::StaleApprovedReview(id)) if id == older_review_id
    ));
    assert!(matches!(
        field_edit,
        Err(ReviewRepositoryError::StaleApprovedReview(id)) if id == older_review_id
    ));
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
        .update_schema_field(newer_review_id, 0, "title", Some(rule("h1.current")))
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
    review_repository
        .approve_review(initial_review_id, None)
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

    review_repository
        .approve_review(append_review_id, None)
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
        .update_schema_field(append_review_id, 1, "title", Some(rule("h1.edited")))
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
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let listing_source_id = ListingSourceId::new();
        insert_listing_source(pool, listing_source_id).await;
        let domain_id = insert_domain(pool, listing_source_id, "example.com").await;
        let url = url::Url::parse("https://example.com/products/waiting").unwrap();
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
        review_repository
            .approve_review(initial_review_id, None)
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_release_pages_backed_off_on_a_schema_review_for_immediate_retry_when_approved() {
    let pool = get_postgres_client().await;
    let fixture = WaitingPageFixture::new(&pool).await;
    assert_eq!(
        fixture.wait(4, time::Duration::hours(4)).await,
        CrawlerUrlWriteOutcome::Applied
    );
    assert!(
        fixture.candidate_urls().await.is_empty(),
        "the backed-off page must wait although cached schemas keep its source eligible"
    );

    fixture
        .review_repository
        .approve_review(fixture.review_id, None)
        .await
        .unwrap();

    assert_eq!(
        fixture.candidate_urls().await,
        vec![fixture.url.clone()],
        "approval must release the backed-off page for an immediate retry"
    );
    assert_eq!(fixture.persisted_wait(&pool).await, (None, 0, None));
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
    fixture
        .review_repository
        .approve_review(fixture.review_id, None)
        .await
        .unwrap();
    assert_eq!(
        fixture.wait(4, time::Duration::hours(4)).await,
        CrawlerUrlWriteOutcome::NoopStale
    );

    assert_eq!(fixture.candidate_urls().await, vec![fixture.url.clone()]);
    assert_eq!(fixture.persisted_wait(&pool).await, (None, 0, None));
}

/// Approving a manual review whose candidate declares removed-page templates appends the
/// templates and releases the pages backed off on the review in one transaction. While the
/// stored templates do not decode, the append fails and the approval changes nothing: the
/// review stays pending and its pages keep their wait. Once the stored row decodes, the same
/// approval does both.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_append_removed_page_templates_and_release_backed_off_pages_in_one_approval() {
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
            json!({
                "schemas": [schema("h1.first")],
                "removed_page_schemas": [notice_template("no longer available")],
            }),
        )
        .await
        .unwrap();

    let refused = fixture
        .review_repository
        .approve_review(fixture.review_id, None)
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
    fixture
        .review_repository
        .approve_review(fixture.review_id, None)
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
        json!([generated, notice_template("no longer available")])
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
    review_repository
        .approve_review(review_id, None)
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
        .update_candidate_payload(review_id, json!({ "schemas": [schema("h1.fixed")] }))
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
        .update_schema_field(review_id, 0, "title", Some(rule("[")))
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
        )
        .await
        .unwrap();
    assert_eq!(matrix.matrix.review_id, Some(review_id));
    assert_eq!(
        matrix.matrix.candidates[0].pages[0].page_reference,
        SchemaPageReference::Persisted { review_page_id }
    );
    repository
        .update_candidate_payload(review_id, json!({ "schemas": [schema("h2")] }))
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
        .store_schema_matrix_if_current(review_id, &matrix)
        .await;
    assert!(matches!(
        stale_store,
        Err(ReviewRepositoryError::CandidateChangedDuringEvaluation)
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
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, &fresh_matrix)
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
        )
        .await
        .unwrap();

    repository
        .store_schema_matrix_if_current(review_id, &evaluated)
        .await
        .unwrap();

    assert_eq!(
        stored_validation_summary(&pool, review_id).await,
        json!({
            "auto_schema_evaluation": { "decision": "APPROVE" },
            "schema_matrix": evaluated.matrix,
        })
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn candidate_change_drops_the_generated_matrix_summary_with_the_cached_matrix() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = generated_schema_review(&pool, &repository).await;

    repository
        .update_candidate_payload(review_id, json!({ "schemas": [schema("h2")] }))
        .await
        .unwrap();

    assert_eq!(
        stored_validation_summary(&pool, review_id).await,
        json!({ "auto_schema_evaluation": { "decision": "APPROVE" } })
    );
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
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "[".to_owned(),
                },
                current_pattern: None,
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

    let result = repository.approve_review(review_id, None).await;

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
        async move { first.approve_review(review_id, None).await },
        async move { second.approve_review(review_id, None).await }
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

    repository.approve_review(review_id, None).await.unwrap();

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

    let undecided_approval = repository.approve_review(review_id, None).await;
    repository
        .update_candidate_payload(
            review_id,
            json!(UrlPatternReviewCandidate {
                decision: UrlPatternDecision::Pattern {
                    value: "/item/".to_owned(),
                },
                current_pattern: None,
            }),
        )
        .await
        .unwrap();
    repository.approve_review(review_id, None).await.unwrap();

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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_repair_request_without_a_note_or_once_the_review_is_closed() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let review_id = pending_schema_review(&repository, &listing_source_id).await;

    for note in [None, Some(" \n ")] {
        assert!(matches!(
            repository.request_repair(review_id, note).await,
            Err(ReviewRepositoryError::MissingRepairNote)
        ));
    }
    let review = repository.get_review(review_id).await.unwrap().review;
    assert_eq!(review.repair_request, None);

    repository.reject_review(review_id, None).await.unwrap();
    assert!(matches!(
        repository.request_repair(review_id, Some("fix the title")).await,
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
        .request_repair(review_id, Some("title misses sold pages"))
        .await
        .unwrap();
    repository
        .request_repair(review_id, Some("title and state miss sold pages"))
        .await
        .unwrap();
    let repaired = schema("h1.product-title");
    repository
        .update_candidate_payload(review_id, json!({ "schemas": [repaired.clone()] }))
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
    repository.approve_review(review_id, None).await.unwrap();

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

/// Approval appends the reviewed removed-page templates to the stored ones once, keeps them on
/// the review, and an approved review cannot change them afterwards.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_append_reviewed_removed_page_templates_on_approval_and_keep_them_fixed() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    insert_listing_source(&pool, listing_source_id).await;
    let generated = RemovedPageSchema {
        selector: "#main h1".into(),
        text: Some("Page not found".to_owned()),
        regex: None,
    };
    let now = OffsetDateTime::now_utc();
    RemovedPageSchemaRepositoryImpl::new(&pool)
        .insert_removed_page_schema(
            &listing_source_id,
            &ListingSourceRemovedPageSchema {
                listing_source_id,
                removed_page_schemas: vec![generated.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    let candidate = json!({
        "schemas": [schema("h1")],
        "removed_page_schemas": [notice_template("sold"), json!(generated)],
    });

    repository
        .update_candidate_payload(review_id, candidate.clone())
        .await
        .unwrap();
    repository.approve_review(review_id, None).await.unwrap();

    let stored = RemovedPageSchemaRepositoryImpl::new(&pool)
        .find_removed_page_schema(&listing_source_id)
        .await
        .unwrap()
        .map(|row| row.removed_page_schemas);
    let reviewed = RemovedPageSchema {
        selector: "p.notice".into(),
        text: Some("sold".to_owned()),
        regex: None,
    };
    assert_eq!(stored, Some(vec![generated, reviewed]));
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.candidate_payload, candidate);

    let changed = json!({
        "schemas": [schema("h1")],
        "removed_page_schemas": [notice_template("no longer available")],
    });
    let refused = repository
        .update_candidate_payload(review_id, changed)
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        ReviewRepositoryError::ApprovedRemovedPageTemplatesChanged(id) if id == review_id
    ));
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
    let review_id = pending_schema_review(&repository, &listing_source_id).await;
    repository
        .update_candidate_payload(
            review_id,
            json!({
                "schemas": [schema("h1")],
                "removed_page_schemas": [notice_template("sold")],
            }),
        )
        .await
        .unwrap();

    let refused = repository
        .approve_review(review_id, None)
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
