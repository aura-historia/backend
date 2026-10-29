use crawler::review::model::{
    CrawlerReviewPage, PAGE_ROLE_PRIMARY, PAGE_ROLE_REVIEWER_ADDED, PAGE_ROLE_SEED,
    PAGE_ROLE_TRIGGERING_GENERATION_PAGE, PAGE_ROLE_WAITING_PAGE, ReviewActor, ReviewEventAction,
    STATUS_PENDING_REVIEW, STATUS_REJECTED, SchemaReviewPageInput,
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
        .check_reviewer_page_target(review_id, ReviewActor::Human, &sold_url)
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
        .evaluate_schema_matrix_for_live_pages(review_id, pages, time::OffsetDateTime::now_utc())
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
            .check_reviewer_page_target(review_id, ReviewActor::Human, &target)
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
        .check_reviewer_page_target(
            review_id,
            ReviewActor::Human,
            &url("https://www.example.com/products/1"),
        )
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
        .check_reviewer_page_target(
            review_id,
            ReviewActor::Human,
            &url("https://example.com/products/2"),
        )
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

/// The spider moves a crawler URL stored without its trailing slash to the slashed spelling it
/// fetches, while a review page keeps the spelling it was recorded under: a rejection holds for
/// its seed and triggering pages in either spelling, and still for no other page.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_treat_rejected_page_as_rejected_in_either_trailing_slash_spelling() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    // The seed page is recorded as https://www.example.com/products/1.
    let (listing_source_id, unslashed_review) = pending_schema_review(&pool).await;
    let sold_url = url("https://www.example.com/products/sold");
    repository
        .add_reviewer_page(
            unslashed_review,
            ReviewActor::Human,
            &sold_url,
            &sold_url,
            SOLD_HTML,
        )
        .await
        .unwrap();
    repository
        .reject_review(
            unslashed_review,
            ReviewActor::Human,
            Some("not a product page"),
        )
        .await
        .unwrap();
    let slashed_review = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            vec![SchemaReviewPageInput {
                url: "https://www.example.com/products/2/".to_owned(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_owned(),
                raw_html: PRODUCT_HTML.to_owned(),
            }],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap();
    repository
        .reject_review(
            slashed_review,
            ReviewActor::Human,
            Some("not a product page"),
        )
        .await
        .unwrap();

    for (crawler_url, rejected) in [
        ("https://www.example.com/products/1", true),
        ("https://www.example.com/products/1/", true),
        ("https://www.example.com/products/2/", true),
        ("https://www.example.com/products/2", true),
        ("https://www.example.com/products/10/", false),
        ("https://www.example.com/products/sold", false),
        ("https://www.example.com/products/sold/", false),
    ] {
        assert_eq!(
            repository
                .has_rejected_schema_review_for_page(
                    &listing_source_id,
                    "manual_schema_required",
                    crawler_url,
                )
                .await
                .unwrap(),
            rejected,
            "{crawler_url}"
        );
    }
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
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            with_html(pages),
            time::OffsetDateTime::now_utc(),
        )
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
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
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
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            with_html(pages),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();

    repository
        .remove_reviewer_page(page.review_page_id, ReviewActor::Human)
        .await
        .unwrap();
    let stored = repository
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
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
        .evaluate_schema_matrix_for_live_pages(
            review_id,
            with_html(pages),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    repository
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
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
            .check_reviewer_page_target(review_id, ReviewActor::Human, &url(raw))
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

/// The crawler URL `url` of the ListingSource's only domain, stored with `url_class` and a retry
/// delay, as the rejection's scrape left a page it classified as other.
async fn insert_crawler_url(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
    url: &str,
    url_class: &str,
) {
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, url_class, raw_source_record_key, next_retry_at) \
         SELECT listing_source_id, domain_id, $2, $3, $2, NOW() + INTERVAL '1 hour' \
         FROM listing_source_domains WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url)
    .bind(url_class)
    .execute(pool)
    .await
    .unwrap();
}

async fn crawler_url_state(pool: &PgPool, url: &str) -> (String, bool) {
    sqlx::query_as::<_, (String, bool)>(
        "SELECT url_class, next_retry_at IS NULL FROM listing_source_urls WHERE url = $1",
    )
    .bind(url)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A human's revocation lifts the rejection's verdict on its seed and triggering pages, in either
/// trailing-slash spelling: the ones the domain's pattern takes become product pages again with no
/// retry delay, the others keep the class the pattern gives them, and pages the rejection never
/// judged keep theirs. The review stays rejected and its trail records the revocation once.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_classify_rejected_pages_again_once_a_human_revokes_the_rejection() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, unrelated_review) = pending_schema_review(&pool).await;
    repository
        .reject_review(unrelated_review, ReviewActor::Human, None)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE listing_source_domains \
         SET url_pattern = '/products/[0-9]+', url_pattern_state = 'MATCHED' \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            vec![
                SchemaReviewPageInput {
                    url: "https://www.example.com/products/2".to_owned(),
                    role: PAGE_ROLE_PRIMARY.to_owned(),
                    raw_html: PRODUCT_HTML.to_owned(),
                },
                SchemaReviewPageInput {
                    url: "https://www.example.com/p/oak-chair-story".to_owned(),
                    role: PAGE_ROLE_SEED.to_owned(),
                    raw_html: PRODUCT_HTML.to_owned(),
                },
            ],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap();
    let reviewer_page = url("https://www.example.com/products/3");
    repository
        .add_reviewer_page(
            review_id,
            ReviewActor::Human,
            &reviewer_page,
            &reviewer_page,
            SOLD_HTML,
        )
        .await
        .unwrap();
    repository
        .reject_review(review_id, ReviewActor::Human, Some("not a product page"))
        .await
        .unwrap();
    for (crawler_url, class) in [
        ("https://www.example.com/products/2/", "other"),
        ("https://www.example.com/p/oak-chair-story", "other"),
        ("https://www.example.com/products/3", "other"),
        ("https://www.example.com/products/1", "other"),
    ] {
        insert_crawler_url(&pool, &listing_source_id, crawler_url, class).await;
    }
    let rejected = |crawler_url: &'static str| {
        let repository = repository.clone();
        async move {
            repository
                .has_rejected_schema_review_for_page(
                    &listing_source_id,
                    "manual_schema_required",
                    crawler_url,
                )
                .await
                .unwrap()
        }
    };
    assert!(rejected("https://www.example.com/products/2/").await);

    let by_agent = repository
        .revoke_rejection(review_id, ReviewActor::AgentReviewer, None)
        .await;
    repository
        .revoke_rejection(
            review_id,
            ReviewActor::Human,
            Some("the agent rejected product pages"),
        )
        .await
        .unwrap();
    let again = repository
        .revoke_rejection(review_id, ReviewActor::Human, None)
        .await;

    assert!(
        matches!(by_agent, Err(ReviewRepositoryError::RevocationNeedsHuman)),
        "{by_agent:?}"
    );
    assert!(
        matches!(again, Err(ReviewRepositoryError::RejectionAlreadyRevoked(id)) if id == review_id),
        "{again:?}"
    );
    for (crawler_url, still_rejected) in [
        ("https://www.example.com/products/2", false),
        ("https://www.example.com/products/2/", false),
        ("https://www.example.com/p/oak-chair-story", false),
        ("https://www.example.com/products/1", true),
    ] {
        assert_eq!(rejected(crawler_url).await, still_rejected, "{crawler_url}");
    }
    assert_eq!(
        crawler_url_state(&pool, "https://www.example.com/products/2/").await,
        ("product".to_owned(), true),
        "a rejected page the pattern takes is scraped again"
    );
    for (crawler_url, why) in [
        (
            "https://www.example.com/p/oak-chair-story",
            "the pattern does not take this rejected page",
        ),
        (
            "https://www.example.com/products/3",
            "the rejection never judged a reviewer-added page",
        ),
        (
            "https://www.example.com/products/1",
            "a page of another rejected review keeps its class",
        ),
    ] {
        assert_eq!(
            crawler_url_state(&pool, crawler_url).await,
            ("other".to_owned(), false),
            "{why}"
        );
    }
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(detail.review.status, STATUS_REJECTED);
    let revocation = detail.events.last().unwrap();
    assert_eq!(
        (
            revocation.actor,
            revocation.action,
            revocation.notes.as_deref()
        ),
        (
            ReviewActor::Human,
            ReviewEventAction::RejectionRevoked,
            Some("the agent rejected product pages")
        )
    );
    assert_eq!(
        revocation.details,
        json!({ "reclassified_urls": ["https://www.example.com/products/2/"] })
    );
    assert_eq!(
        detail
            .events
            .iter()
            .filter(|event| event.action == ReviewEventAction::RejectionRevoked)
            .count(),
        1
    );
}

/// Only rejecting a manual product-schema review classifies pages, so no other review has a
/// rejection to revoke, and a refused revocation records nothing.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_to_revoke_a_review_whose_rejection_classified_no_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (listing_source_id, pending) = pending_schema_review(&pool).await;
    repository
        .reject_review(pending, ReviewActor::Human, None)
        .await
        .unwrap();
    let generated = repository
        .create_schema_review(
            &listing_source_id,
            "initial_schema_generation",
            &[schema()],
            vec![SchemaReviewPageInput {
                url: "https://www.example.com/products/2".to_owned(),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                raw_html: PRODUCT_HTML.to_owned(),
            }],
            json!({}),
        )
        .await
        .unwrap();
    let pending_revocation = repository
        .revoke_rejection(generated, ReviewActor::Human, None)
        .await;
    repository
        .reject_review(generated, ReviewActor::Human, None)
        .await
        .unwrap();

    let generated_revocation = repository
        .revoke_rejection(generated, ReviewActor::Human, None)
        .await;
    let unknown = CrawlerReviewId::new();
    let unknown_revocation = repository
        .revoke_rejection(unknown, ReviewActor::Human, None)
        .await;

    for (revocation, review_id) in [
        (pending_revocation, generated),
        (generated_revocation, generated),
    ] {
        assert!(
            matches!(revocation, Err(ReviewRepositoryError::NoRejectionToRevoke(id)) if id == review_id),
            "{revocation:?}"
        );
    }
    assert!(
        matches!(unknown_revocation, Err(ReviewRepositoryError::NotFound(id)) if id == unknown),
        "{unknown_revocation:?}"
    );
    assert!(
        repository
            .get_review(generated)
            .await
            .unwrap()
            .events
            .iter()
            .all(|event| event.action != ReviewEventAction::RejectionRevoked)
    );
}
