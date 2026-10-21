use crawler::CrawlerReviewId;
use crawler::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_REVIEWER_ADDED, STATUS_PENDING_REVIEW, SchemaReviewPageInput,
};
use crawler::review::repository::{
    CrawlerReviewRepository, MAX_REVIEW_PAGES, ReviewRepositoryError,
};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;
use url::Url;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

const PRODUCT_HTML: &str = "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>";
const SOLD_HTML: &str =
    "<html><body><h1>Sold title</h1><span class=\"state\">Sold</span></body></html>";

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
        title: rule("h1"),
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

fn url(raw: &str) -> Url {
    Url::parse(raw).unwrap()
}

/// A ListingSource on `example.com` with a pending schema review over one product page.
async fn pending_schema_review(pool: &PgPool) -> (ListingSourceId, CrawlerReviewId) {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Test source', 'test-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'example.com', 'www.example.com')",
    )
    .bind(crawler::CrawlerDomainId::new().as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
    let review_id = CrawlerReviewRepository::new(pool.clone())
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            vec![SchemaReviewPageInput {
                url: "https://www.example.com/products/1".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: PRODUCT_HTML.to_string(),
            }],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap();
    (listing_source_id, review_id)
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_evaluate_reviewer_added_page_in_schema_matrix_when_url_is_on_listing_source_domain()
{
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    sqlx::query(
        "UPDATE crawler_reviews SET validation_summary = validation_summary || '{\"schema_matrix\": {}}' \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let sold_url = url("https://example.com/products/sold");

    repository
        .check_reviewer_page_target(review_id, &sold_url)
        .await
        .unwrap();
    let page = repository
        .add_reviewer_page(review_id, &sold_url, SOLD_HTML)
        .await
        .unwrap();

    assert_eq!(page.role, PAGE_ROLE_REVIEWER_ADDED);
    assert_eq!(page.url, sold_url.as_str());
    let detail = repository.get_review(review_id).await.unwrap();
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
    assert_eq!(detail.review.validation_summary["manual_authoring"], true);
    let pages = detail
        .pages
        .into_iter()
        .map(|page| {
            let html = if page.role == PAGE_ROLE_REVIEWER_ADDED {
                SOLD_HTML
            } else {
                PRODUCT_HTML
            };
            (page, html.to_owned())
        })
        .collect();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, pages)
        .await
        .unwrap();
    let evaluated_pages = &evaluated.matrix.candidates[0].pages;
    assert_eq!(evaluated_pages.len(), 2);
    let sold = evaluated_pages
        .iter()
        .find(|page| page.role == PAGE_ROLE_REVIEWER_ADDED)
        .expect("the reviewer page is part of the matrix");
    assert_eq!(sold.url, sold_url.as_str());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_reviewer_page_when_url_is_outside_listing_source_domains() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;

    for raw in [
        "https://example.org/products/1",
        "https://shop.example.com/products/1",
        "https://example.com.evil.test/products/1",
        "ftp://example.com/products/1",
    ] {
        let target = url(raw);
        let checked = repository
            .check_reviewer_page_target(review_id, &target)
            .await;
        assert!(
            matches!(
                checked,
                Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(_))
            ),
            "{raw} must be refused before it is fetched"
        );
        let added = repository
            .add_reviewer_page(review_id, &target, PRODUCT_HTML)
            .await;
        assert!(
            matches!(
                added,
                Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(_))
            ),
            "{raw} must not be stored"
        );
    }
    assert_eq!(
        repository.get_review_pages(review_id).await.unwrap().len(),
        1
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_reviewer_page_when_url_is_already_a_page_or_limit_is_reached() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;

    let duplicate = repository
        .check_reviewer_page_target(review_id, &url("https://www.example.com/products/1"))
        .await;
    assert!(matches!(
        duplicate,
        Err(ReviewRepositoryError::DuplicateReviewPage(_))
    ));

    for index in 1..MAX_REVIEW_PAGES {
        repository
            .add_reviewer_page(
                review_id,
                &url(&format!("https://example.com/products/extra-{index}")),
                PRODUCT_HTML,
            )
            .await
            .unwrap();
    }
    let over_limit = repository
        .add_reviewer_page(
            review_id,
            &url("https://example.com/products/one-too-many"),
            PRODUCT_HTML,
        )
        .await;
    assert!(matches!(
        over_limit,
        Err(ReviewRepositoryError::ReviewPageLimitReached(_))
    ));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_reviewer_page_when_review_is_not_open_for_schema_review() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    repository
        .reject_review(review_id, None, false)
        .await
        .unwrap();

    let result = repository
        .check_reviewer_page_target(review_id, &url("https://example.com/products/2"))
        .await;

    assert!(matches!(result, Err(ReviewRepositoryError::NotPending(_))));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_not_treat_reviewer_page_as_rejected_product_page_when_review_is_rejected() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");
    repository
        .add_reviewer_page(review_id, &sold_url, SOLD_HTML)
        .await
        .unwrap();
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
        .reject_review(review_id, None, false)
        .await
        .unwrap();

    assert!(
        !repository
            .has_rejected_schema_review_for_page(
                &listing_source_id,
                "manual_schema_required",
                sold_url.as_str(),
            )
            .await
            .unwrap()
    );
    assert!(
        repository
            .has_rejected_schema_review_for_page(
                &listing_source_id,
                "manual_schema_required",
                "https://www.example.com/products/1",
            )
            .await
            .unwrap()
    );
}
