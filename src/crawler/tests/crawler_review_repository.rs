use common::shop_id::ShopId;
use crawler::review::model::{PAGE_ROLE_PRIMARY, STATUS_APPROVED, SchemaReviewPageInput};
use crawler::review::repository::CrawlerReviewRepository;
use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, ShopsProductSchema,
};
use crawler::scraper::css_selector::product_schema_repository::{
    ShopsProductSchemaRepository, ShopsProductSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const RDS: Rds = Rds {
    migrations_dir: "src/crawler/migrations",
};

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
        shops_product_id: rule("span.id"),
        title: rule(title_selector),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: rule("span.state"),
        images: image_rule("img.product"),
        auction_start: None,
        auction_end: None,
        default_currency: None,
    }
}

async fn insert_shop(pool: &PgPool, shop_id: ShopId) {
    sqlx::query("INSERT INTO shops (shop_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(Uuid::from(shop_id))
        .execute(pool)
        .await
        .unwrap();
}

#[localstack_test(services = [RDS])]
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
        .create_schema_review_with_status(
            &shop_id,
            "initial_schema_generation",
            &[initial_schema],
            vec![SchemaReviewPageInput {
                url: "https://example.com/products/1".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html><body><span class=\"id\">SKU</span><h1 class=\"new\">Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>".to_string(),
            }],
            json!({
                "auto_schema_evaluation": {
                    "decision": "APPROVE",
                    "confidence": "HIGH",
                    "approved_by_llm": true,
                    "summary": "ok"
                }
            }),
            STATUS_APPROVED,
            Some("Auto-approved by LLM schema evaluator"),
        )
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
