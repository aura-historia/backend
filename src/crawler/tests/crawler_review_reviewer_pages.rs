use crawler::review::model::{
    CrawlerReviewPage, PAGE_ROLE_PRIMARY, PAGE_ROLE_REVIEWER_ADDED, STATUS_PENDING_REVIEW,
    SchemaReviewPageInput,
};
use crawler::review::repository::{
    CrawlerReviewRepository, MAX_REVIEWER_ADDED_PAGES, ReviewRepositoryError,
};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::{CrawlerReviewId, CrawlerReviewPageId};
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
        .add_reviewer_page(review_id, &sold_url, &sold_url, SOLD_HTML)
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
        "https://user:secret@example.com/products/1",
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
            .add_reviewer_page(review_id, &target, &target, PRODUCT_HTML)
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
    let with_fragment = url("https://www.example.com/products/1#gallery");
    let duplicate_with_fragment = repository
        .add_reviewer_page(review_id, &with_fragment, &with_fragment, PRODUCT_HTML)
        .await;
    assert!(matches!(
        duplicate_with_fragment,
        Err(ReviewRepositoryError::DuplicateReviewPage(_))
    ));

    for index in 0..MAX_REVIEWER_ADDED_PAGES {
        let extra = url(&format!("https://example.com/products/extra-{index}"));
        repository
            .add_reviewer_page(review_id, &extra, &extra, PRODUCT_HTML)
            .await
            .unwrap();
    }
    let one_too_many = url("https://example.com/products/one-too-many");
    let over_limit = repository
        .add_reviewer_page(review_id, &one_too_many, &one_too_many, PRODUCT_HTML)
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
        .add_reviewer_page(review_id, &sold_url, &sold_url, SOLD_HTML)
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

fn with_html(pages: Vec<CrawlerReviewPage>) -> Vec<(CrawlerReviewPage, String)> {
    pages
        .into_iter()
        .map(|page| (page, PRODUCT_HTML.to_owned()))
        .collect()
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_to_store_schema_matrix_when_reviewer_page_was_added_during_evaluation() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, with_html(pages))
        .await
        .unwrap();
    let sold_url = url("https://example.com/products/sold");

    repository
        .add_reviewer_page(review_id, &sold_url, &sold_url, SOLD_HTML)
        .await
        .unwrap();
    let stored = repository
        .store_schema_matrix_if_current(review_id, &evaluated)
        .await;

    assert!(matches!(
        stored,
        Err(ReviewRepositoryError::CandidateChangedDuringEvaluation)
    ));
    let detail = repository.get_review(review_id).await.unwrap();
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_remove_reviewer_page_and_cached_matrix_when_reviewer_removes_it() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");
    let page = repository
        .add_reviewer_page(review_id, &sold_url, &sold_url, SOLD_HTML)
        .await
        .unwrap();
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, with_html(pages))
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, &evaluated)
        .await
        .unwrap();

    repository
        .remove_reviewer_page(page.review_page_id)
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.pages.len(), 1);
    assert_eq!(detail.pages[0].role, PAGE_ROLE_PRIMARY);
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_to_remove_review_page_when_reviewer_did_not_add_it() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let primary = repository.get_review_pages(review_id).await.unwrap()[0].review_page_id;

    let removed_primary = repository.remove_reviewer_page(primary).await;
    let removed_unknown = repository
        .remove_reviewer_page(CrawlerReviewPageId::new())
        .await;

    assert!(matches!(
        removed_primary,
        Err(ReviewRepositoryError::NotReviewerAddedPage(id)) if id == primary
    ));
    assert!(matches!(
        removed_unknown,
        Err(ReviewRepositoryError::ReviewPageNotFound(_))
    ));
    assert_eq!(
        repository.get_review_pages(review_id).await.unwrap().len(),
        1
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_reviewer_page_when_shop_redirects_it_to_a_non_product_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");

    let to_homepage = repository
        .add_reviewer_page(
            review_id,
            &sold_url,
            &url("https://example.com/"),
            PRODUCT_HTML,
        )
        .await;
    sqlx::query(
        "UPDATE listing_source_domains \
         SET url_pattern = '/products/', url_pattern_state = 'MATCHED' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let to_category = repository
        .add_reviewer_page(
            review_id,
            &sold_url,
            &url("https://example.com/category/vases"),
            PRODUCT_HTML,
        )
        .await;
    let to_product = repository
        .add_reviewer_page(
            review_id,
            &sold_url,
            &url("https://www.example.com/products/sold-archive"),
            SOLD_HTML,
        )
        .await
        .unwrap();

    assert!(matches!(
        to_homepage,
        Err(ReviewRepositoryError::RedirectedToNonProductPage(_))
    ));
    assert!(matches!(
        to_category,
        Err(ReviewRepositoryError::RedirectedToNonProductPage(_))
    ));
    assert_eq!(
        to_product.url,
        "https://www.example.com/products/sold-archive"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_refuse_reviewer_page_when_it_differs_from_a_review_page_only_by_trailing_slash() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;

    let duplicate = repository
        .check_reviewer_page_target(review_id, &url("https://www.example.com/products/1/"))
        .await;

    assert!(matches!(
        duplicate,
        Err(ReviewRepositoryError::DuplicateReviewPage(_))
    ));
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_not_count_seed_pages_against_reviewer_page_limit() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    for index in 0..MAX_REVIEWER_ADDED_PAGES {
        sqlx::query(
            "INSERT INTO crawler_review_pages (review_page_id, review_id, url, role, html_hash) \
             VALUES ($1, $2, $3, 'SEED', 'hash')",
        )
        .bind(CrawlerReviewPageId::new().as_uuid())
        .bind(review_id.as_uuid())
        .bind(format!("https://example.com/products/seed-{index}"))
        .execute(&pool)
        .await
        .unwrap();
    }

    let sold_url = url("https://example.com/products/sold");
    repository
        .add_reviewer_page(review_id, &sold_url, &sold_url, SOLD_HTML)
        .await
        .unwrap();
}
