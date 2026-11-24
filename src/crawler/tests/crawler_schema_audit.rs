//! The live schema audit's persistence: which ListingSources are due, what it samples, the review
//! it opens and the findings an approval accepted. The audit's judgement is unit-tested in
//! `review::schema_audit`; the hold on new listings in `tests/candidates.rs`.

use crawler::review::model::{PAGE_ROLE_SEED, STATUS_PENDING_REVIEW, SchemaReviewPageInput};
use crawler::review::repository::{CrawlerReviewRepository, SchemaAuditSource};
use crawler::review::schema_audit::SCHEMA_AUDIT_REASON;
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use listing_source_core::ListingSourceId;
use sqlx::PgPool;
use std::time::Duration;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn schema(title_selector: &str) -> ProductCssSelectorSchema {
    let rule = |selector: &str| ExtractionRule {
        selector: selector.into(),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    };
    ProductCssSelectorSchema {
        source_listing_id: Some(rule("span.id")),
        title: Some(rule(title_selector)),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: Some(rule("span.state")),
        images: Some(ExtractionRule {
            extract: ExtractionKind::ImageUrl,
            cardinality: ExtractionCardinality::All,
            ..rule("img")
        }),
        raw_attributes: Default::default(),
    }
}

/// A crawl-enabled ListingSource named `name` on `host`, with `live_schemas` stored unless empty.
async fn insert_source(
    pool: &PgPool,
    name: &str,
    host: &str,
    live_schemas: &[ProductCssSelectorSchema],
) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, created, updated) \
         VALUES ($1, $2, $3, TRUE, $4, $3, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(name)
    .bind(host)
    .bind(format!("https://{host}/"))
    .execute(pool)
    .await
    .unwrap();
    if !live_schemas.is_empty() {
        sqlx::query(
            "INSERT INTO listing_source_product_schemas \
             (listing_source_id, product_schema, created, updated) VALUES ($1, $2, NOW(), NOW())",
        )
        .bind(listing_source_id.as_uuid())
        .bind(serde_json::to_value(live_schemas).unwrap())
        .execute(pool)
        .await
        .unwrap();
    }
    listing_source_id
}

async fn set_audited_days_ago(pool: &PgPool, listing_source_id: ListingSourceId, days: i32) {
    sqlx::query(
        "UPDATE listing_sources SET schema_audited_at = NOW() - make_interval(days => $2) \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(days)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_product_url(
    pool: &PgPool,
    listing_source_id: ListingSourceId,
    url: &str,
    captured_identity: Option<&str>,
) {
    UrlMetadataRepositoryImpl::new(pool.clone())
        .upsert_link(
            &listing_source_id,
            &url::Url::parse(url).unwrap(),
            &UrlClass::ProductListing,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE listing_source_urls SET last_source_listing_id = $2 WHERE url = $1")
        .bind(url)
        .bind(captured_identity)
        .execute(pool)
        .await
        .unwrap();
}

fn audited_pages(host: &str) -> Vec<SchemaReviewPageInput> {
    vec![SchemaReviewPageInput {
        url: format!("https://{host}/p/1"),
        role: PAGE_ROLE_SEED.to_owned(),
        raw_html:
            "<html><head><title>Chair | Shop</title></head><body><h1>Chair</h1></body></html>"
                .to_owned(),
    }]
}

fn findings() -> Vec<String> {
    vec![
        "title LISTING_SOURCE_NAME".to_owned(),
        "title READS_DOCUMENT_TITLE".to_owned(),
    ]
}

async fn set_review_status(pool: &PgPool, review_id: crawler::CrawlerReviewId, status: &str) {
    sqlx::query("UPDATE crawler_reviews SET status = $2, updated = NOW() WHERE review_id = $1")
        .bind(review_id.as_uuid())
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

/// Sources never audited come first, then the longest ago; a source audited within the audit
/// age, one without live schemas, one with a pending schema review and one not crawled are not
/// due. An audit it records moves the source out of the due set.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_take_the_sources_never_audited_first_and_skip_those_not_due() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("h1")];
    let never = insert_source(&pool, "Never", "never.example", &live).await;
    let long_ago = insert_source(&pool, "Long ago", "long-ago.example", &live).await;
    set_audited_days_ago(&pool, long_ago, 8).await;
    let recent = insert_source(&pool, "Recent", "recent.example", &live).await;
    set_audited_days_ago(&pool, recent, 1).await;
    insert_source(&pool, "No schemas", "no-schemas.example", &[]).await;
    let pending = insert_source(&pool, "Pending", "pending.example", &live).await;
    repository
        .open_schema_audit_review(
            &pending,
            &live,
            audited_pages("pending.example"),
            &findings(),
        )
        .await
        .unwrap();
    let disabled = insert_source(&pool, "Disabled", "disabled.example", &live).await;
    sqlx::query("UPDATE listing_sources SET crawl_enabled = FALSE WHERE listing_source_id = $1")
        .bind(disabled.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let due = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();
    repository.record_schema_audit(&never).await.unwrap();
    let due_after_audit = repository.schema_audit_due_sources(WEEK, 10).await.unwrap();

    let source = |listing_source_id, name: &str| SchemaAuditSource {
        listing_source_id,
        listing_source_name: name.to_owned(),
    };
    assert_eq!(
        due,
        vec![source(never, "Never"), source(long_ago, "Long ago")]
    );
    assert_eq!(due_after_audit, vec![source(long_ago, "Long ago")]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_sample_the_live_schemas_and_only_captured_product_urls() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    insert_product_url(&pool, source, "https://shop.example/p/1", Some("SKU-1")).await;
    insert_product_url(&pool, source, "https://shop.example/p/new", None).await;

    let (schemas, urls) = repository.schema_audit_sample(&source).await.unwrap();

    assert_eq!(schemas, live);
    assert_eq!(
        urls.iter()
            .map(|page| (page.url.as_str(), page.captured_identity.as_str()))
            .collect::<Vec<_>>(),
        vec![("https://shop.example/p/1", "SKU-1")]
    );
}

/// The review's candidate is the live schemas as they stand, and it records the findings, which
/// the review tools show the agents.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_pending_review_of_the_live_schemas_that_records_the_findings() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;

    let review_id = repository
        .open_schema_audit_review(&source, &live, audited_pages("shop.example"), &findings())
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_PENDING_REVIEW);
    assert_eq!(detail.review.reason, SCHEMA_AUDIT_REASON);
    assert_eq!(
        detail.review.candidate_payload["schemas"],
        serde_json::to_value(live).unwrap()
    );
    assert_eq!(
        detail.review.validation_summary["schema_audit"]["findings"],
        serde_json::json!(findings())
    );
    assert_eq!(
        detail
            .pages
            .iter()
            .map(|page| page.role.as_str())
            .collect::<Vec<_>>(),
        vec![PAGE_ROLE_SEED]
    );
    assert_eq!(repository.pending_schema_audit_reviews().await.unwrap(), 1);
}

/// An approval that made exactly the live schemas live judged them with the findings its review
/// was opened with; a pending review, or live schemas changed since, accepted nothing.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_accept_the_findings_of_an_approved_audit_review_of_the_same_live_schemas() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let live = [schema("title")];
    let source = insert_source(&pool, "Shop", "shop.example", &live).await;
    let review_id = repository
        .open_schema_audit_review(&source, &live, audited_pages("shop.example"), &findings())
        .await
        .unwrap();

    let while_pending = repository
        .accepted_schema_audit_findings(&source, &live)
        .await
        .unwrap();
    set_review_status(&pool, review_id, "APPROVED").await;
    let approved = repository
        .accepted_schema_audit_findings(&source, &live)
        .await
        .unwrap();
    let other_schemas = repository
        .accepted_schema_audit_findings(&source, &[schema("h1")])
        .await
        .unwrap();

    assert_eq!(while_pending, None);
    assert_eq!(approved, Some(findings()));
    assert_eq!(other_schemas, None);
}

/// A pass audits each due source on its captured pages as the shared fetcher serves them: a title
/// rule on the document `<title>` that adds the shop's name opens an audit review, while a rule on
/// the heading only records the audit.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_open_a_review_only_for_the_source_whose_live_schemas_fail_the_audit() {
    use crawler::scraper::scraper_service::{FetchedHtml, MockHtmlFetcher};
    use crawler::service::cron::CronStop;
    use crawler::service::schema_audit::SchemaAudit;
    use std::sync::Arc;

    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let failing = insert_source(
        &pool,
        "Bauman Rare Books",
        "bauman.example",
        &[schema("title")],
    )
    .await;
    let passing = insert_source(&pool, "Heading Shop", "heading.example", &[schema("h1")]).await;
    for (source, host) in [(failing, "bauman.example"), (passing, "heading.example")] {
        for item in ["suttree", "human-condition"] {
            insert_product_url(
                &pool,
                source,
                &format!("https://{host}/p/{item}"),
                Some(item),
            )
            .await;
        }
    }
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().returning(|url| {
        let item = url.path().trim_start_matches("/p/").to_owned();
        let html = format!(
            "<html><head><title>{item} First Edition - Bauman Rare Books</title></head><body>\
             <h1>{item}</h1><span class=\"id\">{item}</span><span class=\"state\">In stock</span>\
             <img src=\"/images/{item}.jpg\"></body></html>"
        );
        let url = url.clone();
        Box::pin(async move { Ok(FetchedHtml::new(html, url, reqwest::StatusCode::OK)) })
    });
    let audit = SchemaAudit::new(repository.clone(), Arc::new(fetcher));

    audit.run_once(&CronStop::default()).await;

    let reasons: Vec<(uuid::Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT listing_source_id, reason, validation_summary->'schema_audit'->'findings' \
         FROM crawler_reviews",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        reasons,
        vec![(
            *failing.as_uuid(),
            SCHEMA_AUDIT_REASON.to_owned(),
            serde_json::json!(["title LISTING_SOURCE_NAME", "title READS_DOCUMENT_TITLE"])
        )]
    );
    assert_eq!(
        repository.schema_audit_due_sources(WEEK, 10).await.unwrap(),
        vec![],
        "both sources are audited"
    );
}
