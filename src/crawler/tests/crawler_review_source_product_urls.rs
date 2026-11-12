//! The product URLs of a review's ListingSource that the review read offers its author.

use crawler::CrawlerReviewId;
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, STATUS_PENDING_REVIEW, SchemaReviewPageInput, SourceProductUrl,
};
use crawler::review::readers::{MAX_SOURCE_PRODUCT_URLS, SourceProductUrlReader};
use crawler::review::repository::{CrawlerReviewRepository, SchemaReviewWithStatusInput};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use time::{Duration, OffsetDateTime};

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
    }
}

async fn listing_source(pool: &PgPool, slug: &str) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, $2, $2, TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(slug)
    .execute(pool)
    .await
    .unwrap();
    listing_source_id
}

async fn schema_review(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    page_url: &str,
) -> CrawlerReviewId {
    CrawlerReviewRepository::new(pool.clone())
        .create_schema_review_with_status(SchemaReviewWithStatusInput {
            listing_source_id: &listing_source_id,
            reason: "manual_schema_required",
            schemas: &[schema()],
            pages: vec![SchemaReviewPageInput {
                url: page_url.to_owned(),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                raw_html: "<html><body><h1>Title</h1></body></html>".to_owned(),
            }],
            validation_summary: json!({}),
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
        .unwrap()
}

async fn stored_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    url: &str,
    url_class: &str,
    disposition: &str,
    scraped_minutes_ago: Option<i64>,
    error_kind: Option<&str>,
) {
    let last_scraped =
        scraped_minutes_ago.map(|minutes| OffsetDateTime::now_utc() - Duration::minutes(minutes));
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, url, url_class, crawler_disposition, last_scraped, last_error_kind, raw_source_record_key) \
         VALUES ($1, $2, $3, $4, $5, $6, $2)",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url)
    .bind(url_class)
    .bind(disposition)
    .bind(last_scraped)
    .bind(error_kind)
    .execute(pool)
    .await
    .unwrap();
}

async fn offered(pool: &PgPool, review_id: CrawlerReviewId) -> Vec<SourceProductUrl> {
    SourceProductUrlReader::new(pool.clone())
        .list(review_id)
        .await
        .unwrap()
}

fn urls(offered: &[SourceProductUrl]) -> Vec<&str> {
    offered.iter().map(|url| url.url.as_str()).collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_offer_scraped_urls_first_by_recency_then_the_rest_without_the_review_pages() {
    let pool = get_postgres_client().await;
    let source = listing_source(&pool, "shop").await;
    let review_id = schema_review(&pool, source, "https://shop.example/review-page").await;
    let other = listing_source(&pool, "other").await;
    for (url, class, disposition, minutes, error) in [
        (
            "https://shop.example/old-ok",
            "product",
            "ACTIVE",
            Some(90),
            None,
        ),
        (
            "https://shop.example/new-ok",
            "product",
            "DORMANT_SOLD",
            Some(5),
            None,
        ),
        (
            "https://shop.example/failed",
            "product",
            "ACTIVE",
            Some(1),
            Some("MissingRequiredValue"),
        ),
        (
            "https://shop.example/never-b",
            "product",
            "ACTIVE",
            None,
            None,
        ),
        (
            "https://shop.example/never-a",
            "product",
            "ACTIVE",
            None,
            None,
        ),
        (
            "https://shop.example/review-page",
            "product",
            "ACTIVE",
            Some(2),
            None,
        ),
        (
            "https://shop.example/category",
            "category",
            "ACTIVE",
            Some(3),
            None,
        ),
        (
            "https://shop.example/duplicate",
            "product",
            "DUPLICATE",
            Some(4),
            None,
        ),
    ] {
        stored_url(&pool, source, url, class, disposition, minutes, error).await;
    }
    stored_url(
        &pool,
        other,
        "https://other.example/item",
        "product",
        "ACTIVE",
        Some(1),
        None,
    )
    .await;

    let offered = offered(&pool, review_id).await;

    assert_eq!(
        urls(&offered),
        [
            "https://shop.example/new-ok",
            "https://shop.example/old-ok",
            "https://shop.example/failed",
            "https://shop.example/never-a",
            "https://shop.example/never-b",
        ]
    );
    assert_eq!(
        offered[2].last_error_kind.as_deref(),
        Some("MissingRequiredValue")
    );
    assert!(offered[3].last_scraped.is_none());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_offer_at_most_forty_urls_keeping_the_most_recently_scraped() {
    let pool = get_postgres_client().await;
    let source = listing_source(&pool, "shop").await;
    let review_id = schema_review(&pool, source, "https://shop.example/review-page").await;
    for index in 0..50 {
        stored_url(
            &pool,
            source,
            &format!("https://shop.example/item-{index:02}"),
            "product",
            "ACTIVE",
            Some(i64::from(index) + 1),
            None,
        )
        .await;
    }

    let offered = offered(&pool, review_id).await;

    assert_eq!(offered.len() as i64, MAX_SOURCE_PRODUCT_URLS);
    assert_eq!(offered[0].url, "https://shop.example/item-00");
    assert_eq!(offered[39].url, "https://shop.example/item-39");
}
