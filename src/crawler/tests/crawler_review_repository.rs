use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, STATUS_APPROVED,
    STATUS_PENDING_REVIEW, SchemaReviewPageInput,
};
use crawler::review::repository::{CrawlerReviewRepository, SchemaReviewWithStatusInput};
use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, ShopsProductSchema,
};
use crawler::scraper::css_selector::product_schema_repository::{
    ShopsProductSchemaRepository, ShopsProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use regex::Regex;
use serde_json::json;
use shop_core::shop_id::ShopId;
use sqlx::PgPool;
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
        shops_product_id: Some(rule("span.id")),
        title: rule(title_selector),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        seller_name: None,
        state: rule("span.state"),
        images: image_rule("img.product"),
        auction_start: None,
        auction_end: None,
        default_currency: None,
        raw_attributes: Default::default(),
    }
}

async fn insert_shop(pool: &PgPool, shop_id: ShopId) {
    sqlx::query("INSERT INTO shops (shop_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(Uuid::from(shop_id))
        .execute(pool)
        .await
        .unwrap();
}

fn review_pages() -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: "https://example.com/products/1".to_string(),
        role: PAGE_ROLE_PRIMARY.to_string(),
        raw_html: "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
    }]
}

#[aura_integration_test(services = [POSTGRES])]
async fn fresh_generation_review_page_role_is_persisted() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            shop_id: &shop_id,
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

async fn pending_review_count(pool: &PgPool, shop_id: ShopId, artifact_type: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM crawler_reviews
         WHERE shop_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'",
    )
    .bind(Uuid::from(shop_id))
    .bind(artifact_type)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn review_page_count(pool: &PgPool, review_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_pages WHERE review_id = $1")
        .bind(review_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn review_url_count(pool: &PgPool, review_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM crawler_review_urls WHERE review_id = $1")
        .bind(review_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [POSTGRES])]
async fn approved_schema_candidate_edit_updates_live_schema_and_audit_payload() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let schema_repository = ShopsProductSchemaRepositoryImpl::new(&pool);
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    let initial_schema = schema("h1.old");
    let now = OffsetDateTime::now_utc();
    schema_repository
        .insert_product_schema(
            &shop_id,
            &ShopsProductSchema {
                shop_id,
                product_schemas: vec![initial_schema.clone()],
                created: now,
                updated: now,
            },
        )
        .await
        .unwrap();

    let review_id = review_repository
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            shop_id: &shop_id,
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
        .find_product_schema(&shop_id)
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

#[aura_integration_test(services = [POSTGRES])]
async fn concurrent_schema_reviews_return_same_pending_review_without_duplicate_pages() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    let schema = schema("h1");
    let first_repo = review_repository.clone();
    let first_schema = schema.clone();
    let second_repo = review_repository.clone();
    let second_schema = schema.clone();

    let (first, second) = tokio::join!(
        async move {
            first_repo
                .create_schema_review(
                    &shop_id,
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
                    &shop_id,
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
        pending_review_count(&pool, shop_id, "PRODUCT_SCHEMA").await,
        1
    );
    assert_eq!(review_page_count(&pool, first_id).await, 1);
}

#[aura_integration_test(services = [POSTGRES])]
async fn concurrent_url_pattern_reviews_return_same_pending_review_without_duplicate_urls() {
    let pool = get_postgres_client().await;
    let review_repository = CrawlerReviewRepository::new(pool.clone());
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

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
                    &shop_id,
                    None,
                    "url_pattern_generation",
                    Some(&pattern),
                    &first_urls,
                    None,
                )
                .await
        },
        async move {
            let pattern = Regex::new("/product/").unwrap();
            second_repo
                .create_url_pattern_review(
                    &shop_id,
                    None,
                    "url_pattern_generation",
                    Some(&pattern),
                    &second_urls,
                    None,
                )
                .await
        }
    );

    let first_id = first.unwrap();
    let second_id = second.unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(pending_review_count(&pool, shop_id, "URL_PATTERN").await, 1);
    assert_eq!(review_url_count(&pool, first_id).await, 2);
}
