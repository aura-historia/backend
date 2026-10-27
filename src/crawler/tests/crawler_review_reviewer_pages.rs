use crawler::review::model::{
    CrawlerReviewPage, PAGE_ROLE_PRIMARY, PAGE_ROLE_REVIEWER_ADDED,
    PAGE_ROLE_TRIGGERING_GENERATION_PAGE, PAGE_ROLE_WAITING_PAGE, ReviewActor,
    STATUS_PENDING_REVIEW, SchemaReviewPageInput,
};
use crawler::review::repository::{
    CrawlerReviewRepository, MAX_REVIEWER_ADDED_PAGES, MAX_WAITING_PAGES, ReviewRepositoryError,
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
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
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
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
            .add_reviewer_page(
                review_id,
                ReviewActor::Human,
                &target,
                &target,
                PRODUCT_HTML,
            )
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
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
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &with_fragment,
            &with_fragment,
            PRODUCT_HTML,
        )
        .await;
    assert!(matches!(
        duplicate_with_fragment,
        Err(ReviewRepositoryError::DuplicateReviewPage(_))
    ));

    for index in 0..MAX_REVIEWER_ADDED_PAGES {
        let extra = url(&format!("https://example.com/products/extra-{index}"));
        repository
            .add_reviewer_page(review_id, ReviewActor::Human, &extra, &extra, PRODUCT_HTML)
            .await
            .unwrap();
    }
    let one_too_many = url("https://example.com/products/one-too-many");
    let over_limit = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &one_too_many,
            &one_too_many,
            PRODUCT_HTML,
        )
        .await;
    assert!(matches!(
        over_limit,
        Err(ReviewRepositoryError::ReviewPageLimitReached(_))
    ));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_reviewer_page_when_review_is_not_open_for_schema_review() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    repository
        .reject_review(review_id, ReviewActor::Human, None)
        .await
        .unwrap();

    let result = repository
        .check_reviewer_page_target(review_id, &url("https://example.com/products/2"))
        .await;

    assert!(matches!(result, Err(ReviewRepositoryError::NotPending(_))));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_treat_reviewer_page_as_rejected_product_page_when_review_is_rejected() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");
    repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
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
        .reject_review(review_id, ReviewActor::Human, None)
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

/// A page added while the matrix is evaluated, by a reviewer or as a waiting page, does not fail
/// the evaluation: the matrix of the pages it evaluated is stored and names them, and the next
/// evaluation covers the new page.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_store_the_matrix_of_the_evaluated_pages_when_a_page_was_added_during_evaluation() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated_page_id = pages[0].review_page_id;
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, with_html(pages))
        .await
        .unwrap();
    let sold_url = url("https://example.com/products/sold");

    repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, &evaluated)
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.pages.len(), 2);
    let stored_page_ids: Vec<&serde_json::Value> =
        detail.review.validation_summary["schema_matrix"]["candidates"][0]["pages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|page| &page["page_reference"]["review_page_id"])
            .collect();
    assert_eq!(stored_page_ids, vec![&json!(evaluated_page_id)]);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_store_schema_matrix_when_an_evaluated_page_was_removed_during_evaluation()
{
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");
    let page = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
        .await
        .unwrap();
    let pages = repository.get_review_pages(review_id).await.unwrap();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, with_html(pages))
        .await
        .unwrap();

    repository
        .remove_reviewer_page(page.review_page_id, ReviewActor::Human)
        .await
        .unwrap();
    let stored = repository
        .store_schema_matrix_if_current(review_id, &evaluated)
        .await;

    assert!(matches!(
        stored,
        Err(ReviewRepositoryError::CandidateChangedDuringEvaluation {
            candidate_version: 1
        })
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_remove_reviewer_page_and_cached_matrix_when_reviewer_removes_it() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");
    let page = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
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
        .remove_reviewer_page(page.review_page_id, ReviewActor::Human)
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_drop_generated_matrix_summary_with_cached_matrix_when_reviewer_adds_a_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    sqlx::query(
        "UPDATE crawler_reviews SET validation_summary = validation_summary \
         || '{\"schema_matrix\": {}, \"schema_matrix_summary\": {}}' \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let sold_url = url("https://example.com/products/sold");

    repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
        .await
        .unwrap();

    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail.review.validation_summary,
        json!({ "manual_authoring": true })
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_remove_review_page_when_reviewer_did_not_add_it() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;
    let primary = repository.get_review_pages(review_id).await.unwrap()[0].review_page_id;

    let removed_primary = repository
        .remove_reviewer_page(primary, ReviewActor::Human)
        .await;
    let removed_unknown = repository
        .remove_reviewer_page(CrawlerReviewPageId::new(), ReviewActor::Human)
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_reviewer_page_when_shop_redirects_it_to_a_non_product_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let sold_url = url("https://example.com/products/sold");

    let to_homepage = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
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
            ReviewActor::Human,
            &sold_url,
            &url("https://example.com/category/vases"),
            PRODUCT_HTML,
        )
        .await;
    let to_product = repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
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

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_reviewer_page_when_it_differs_from_a_review_page_only_in_spelling() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id) = pending_schema_review(&pool).await;

    for raw in [
        "https://www.example.com/products/1/",
        "https://example.com/products/1",
        "http://www.example.com/products/1",
    ] {
        let duplicate = repository
            .check_reviewer_page_target(review_id, &url(raw))
            .await;

        assert!(
            matches!(
                duplicate,
                Err(ReviewRepositoryError::DuplicateReviewPage(_))
            ),
            "{raw} names the primary page"
        );
    }
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
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
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Pages waiting on a pending review
// ---------------------------------------------------------------------------

async fn mark_schema_matrix_cached(pool: &PgPool, review_id: CrawlerReviewId) {
    sqlx::query(
        "UPDATE crawler_reviews SET validation_summary = validation_summary || '{\"schema_matrix\": {}}' \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

fn pages_with_role(pages: &[CrawlerReviewPage], role: &str) -> Vec<String> {
    pages
        .iter()
        .filter(|page| page.role == role)
        .map(|page| page.url.clone())
        .collect()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_attach_waiting_page_once_and_drop_cached_matrix_when_schema_review_is_pending() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    mark_schema_matrix_cached(&pool, review_id).await;
    let waiting_url = url("https://example.com/products/archived");

    let joined = repository
        .join_pending_schema_review(&listing_source_id, &waiting_url, SOLD_HTML)
        .await
        .unwrap();
    // The same page under another spelling joins again without a second attachment.
    let rejoined = repository
        .join_pending_schema_review(
            &listing_source_id,
            &url("https://www.example.com/products/archived/"),
            SOLD_HTML,
        )
        .await
        .unwrap();

    assert_eq!(joined, Some(review_id));
    assert_eq!(rejoined, Some(review_id));
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        pages_with_role(&detail.pages, PAGE_ROLE_WAITING_PAGE),
        vec![waiting_url.to_string()]
    );
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_attach_at_most_max_waiting_pages_when_more_pages_wait_on_schema_review() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;

    for index in 0..=MAX_WAITING_PAGES {
        let joined = repository
            .join_pending_schema_review(
                &listing_source_id,
                &url(&format!("https://example.com/products/waiting-{index}")),
                SOLD_HTML,
            )
            .await
            .unwrap();
        assert_eq!(joined, Some(review_id));
    }

    let pages = repository.get_review_pages(review_id).await.unwrap();
    assert_eq!(
        pages_with_role(&pages, PAGE_ROLE_WAITING_PAGE).len(),
        MAX_WAITING_PAGES
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_join_when_no_product_schema_review_is_pending() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    repository
        .reject_review(review_id, ReviewActor::Human, Some("not a product page"))
        .await
        .unwrap();

    let joined = repository
        .join_pending_schema_review(
            &listing_source_id,
            &url("https://example.com/products/archived"),
            SOLD_HTML,
        )
        .await
        .unwrap();

    assert_eq!(joined, None);
    let pages = repository.get_review_pages(review_id).await.unwrap();
    assert!(pages_with_role(&pages, PAGE_ROLE_WAITING_PAGE).is_empty());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_attach_pages_of_a_joining_schema_review_as_waiting_pages() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let waiting_url = "https://example.com/products/archived";

    let joined = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            vec![SchemaReviewPageInput {
                url: waiting_url.to_string(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_string(),
                raw_html: SOLD_HTML.to_string(),
            }],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap();

    assert_eq!(joined, review_id);
    let pages = repository.get_review_pages(review_id).await.unwrap();
    assert_eq!(
        pages_with_role(&pages, PAGE_ROLE_WAITING_PAGE),
        vec![waiting_url.to_string()]
    );
    assert!(pages_with_role(&pages, PAGE_ROLE_TRIGGERING_GENERATION_PAGE).is_empty());
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_remove_waiting_page_when_reviewer_removes_it() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    repository
        .join_pending_schema_review(
            &listing_source_id,
            &url("https://example.com/products/archived"),
            SOLD_HTML,
        )
        .await
        .unwrap();
    let waiting = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .find(|page| page.role == PAGE_ROLE_WAITING_PAGE)
        .unwrap();

    repository
        .remove_reviewer_page(waiting.review_page_id, ReviewActor::Human)
        .await
        .unwrap();

    let pages = repository.get_review_pages(review_id).await.unwrap();
    assert!(pages_with_role(&pages, PAGE_ROLE_WAITING_PAGE).is_empty());
    assert_eq!(pages.len(), 1);
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_not_treat_rejection_as_verdict_on_waiting_pages() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, review_id) = pending_schema_review(&pool).await;
    let waiting_url = "https://example.com/products/archived";
    repository
        .join_pending_schema_review(&listing_source_id, &url(waiting_url), SOLD_HTML)
        .await
        .unwrap();

    repository
        .reject_review(review_id, ReviewActor::Human, Some("not a product page"))
        .await
        .unwrap();

    assert!(
        !repository
            .has_rejected_schema_review_for_page(
                &listing_source_id,
                "manual_schema_required",
                waiting_url,
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
