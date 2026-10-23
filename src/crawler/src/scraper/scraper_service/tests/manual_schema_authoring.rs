use super::*;
use crate::authoring::CrawlerAuthoringMode;
use crate::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, PAGE_ROLE_WAITING_PAGE,
    SchemaReviewPageInput,
};
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use crate::scraper::scraper_service::{ScrapedProduct, ScraperError};
use serde_json::json;
use sqlx::PgPool;
use test_api::IntegrationTestService as _;

async fn insert_listing_source(pool: &PgPool) -> ListingSourceId {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Manual source', 'manual-source', TRUE, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert listing source: {error}"));
    listing_source_id
}

fn fetcher_returning_sample_page() -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));
    fetcher
}

/// A schema service that serves `cached` and fails the test if anything asks it to generate.
fn schema_service_without_generation(
    cached: Option<ListingSourceProductSchema>,
) -> MockProductListingSchemaService {
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().returning(move |_| {
        let cached = cached.clone();
        Box::pin(async move { Ok(cached) })
    });
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    schema_svc
}

/// A fetcher serving the sample product page from `url` `times` times.
fn fetcher_returning_sample_page_at(url: &Url, times: usize) -> MockHtmlFetcher {
    let final_url = url.clone();
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().times(times).returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });
    fetcher
}

fn scraper_with_review_gate(
    fetcher: MockHtmlFetcher,
    schema_svc: MockProductListingSchemaService,
    review_repository: CrawlerReviewRepository,
    candidates: MockScraperCandidateService,
    authoring: CrawlerAuthoringMode,
) -> ScraperServiceImpl {
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(candidates),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_review_gate(review_repository, authoring)
}

fn manual_scraper(
    schema_svc: MockProductListingSchemaService,
    review_repository: CrawlerReviewRepository,
    candidates: MockScraperCandidateService,
) -> ScraperServiceImpl {
    scraper_with_review_gate(
        fetcher_returning_sample_page(),
        schema_svc,
        review_repository,
        candidates,
        CrawlerAuthoringMode::Manual,
    )
}

fn cached_schemas(
    listing_source_id: ListingSourceId,
    product_schemas: Vec<ProductCssSelectorSchema>,
) -> ListingSourceProductSchema {
    ListingSourceProductSchema {
        listing_source_id,
        product_schemas,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

/// A schema whose title selector no longer matches the sample page.
fn stale_schema() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.title.selector = CssSelector::from("h1.no-longer-present");
    schema
}

/// Opens the pending product-schema review another page of the ListingSource triggered, with a
/// cached schema matrix.
async fn open_pending_schema_review(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
) -> crate::CrawlerReviewId {
    repository
        .create_schema_review(
            listing_source_id,
            MANUAL_SCHEMA_REASON,
            &[minimal_schema()],
            vec![SchemaReviewPageInput {
                url: "https://example.com/products/first-template".to_owned(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_owned(),
                raw_html: sample_html(),
            }],
            json!({ "schema_matrix": {} }),
        )
        .await
        .unwrap_or_else(|error| panic!("open pending review: {error}"))
}

fn pending_review_id(
    result: Result<Option<ScrapedProduct>, ScraperError>,
) -> crate::CrawlerReviewId {
    match result {
        Err(ScraperError::PendingSchemaReview { review_id, .. }) => review_id,
        Err(error) => panic!("expected a pending manual schema review, got {error}"),
        Ok(_) => panic!("expected a pending manual schema review, got a scraped product"),
    }
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_open_manual_schema_review_instead_of_generating_initial_schema() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let service = manual_scraper(
        schema_service_without_generation(None),
        repository.clone(),
        MockScraperCandidateService::new(),
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    let review = repository
        .get_review(pending_review_id(result))
        .await
        .unwrap_or_else(|error| panic!("read manual review: {error}"));
    assert_eq!(review.review.reason, MANUAL_SCHEMA_REASON);
    assert_eq!(
        review.review.candidate_payload["schemas"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(review.pages.len(), 1);
    assert_eq!(review.pages[0].role, PAGE_ROLE_PRIMARY);
    assert_eq!(review.pages[0].url, url.as_str());
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_keep_cached_schemas_in_manual_review_when_they_stop_matching() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let stale_schema = stale_schema();
    let cached = cached_schemas(id, vec![stale_schema.clone()]);
    let repository = CrawlerReviewRepository::new(pool.clone());
    let service = manual_scraper(
        schema_service_without_generation(Some(cached)),
        repository.clone(),
        MockScraperCandidateService::new(),
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    let review = repository
        .get_review(pending_review_id(result))
        .await
        .unwrap_or_else(|error| panic!("read manual review: {error}"));
    let schemas: Vec<ProductCssSelectorSchema> =
        serde_json::from_value(review.review.candidate_payload["schemas"].clone())
            .unwrap_or_else(|error| panic!("manual review schemas: {error}"));
    assert_eq!(review.review.reason, MANUAL_SCHEMA_REASON);
    assert_eq!(schemas.len(), 2);
    assert_eq!(schemas[0], stale_schema);
    assert_eq!(review.pages[0].role, PAGE_ROLE_TRIGGERING_GENERATION_PAGE);
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_classify_page_as_other_instead_of_reopening_a_rejected_manual_review() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let opening = manual_scraper(
        schema_service_without_generation(None),
        repository.clone(),
        MockScraperCandidateService::new(),
    );
    let rejected = pending_review_id(opening.scrape(primary_only_request(&id, &url)).await.result);
    repository
        .reject_review(rejected, Some("not a product page"), false)
        .await
        .unwrap_or_else(|error| panic!("reject manual review: {error}"));
    let mut candidates = MockScraperCandidateService::new();
    let expected_url = url.clone();
    candidates
        .expect_set_class()
        .once()
        .withf(move |_, received_url, url_class, _| {
            received_url == &expected_url
                && *url_class == crate::spider::classification::url_metadata::UrlClass::Other
        })
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    let rescraping = manual_scraper(
        schema_service_without_generation(None),
        repository.clone(),
        candidates,
    );

    let result = rescraping
        .scrape(primary_only_request(&id, &url))
        .await
        .result;

    assert!(matches!(result, Err(ScraperError::NotProductPage { .. })));
    assert_eq!(
        repository
            .latest_pending_review_id(&id, "PRODUCT_SCHEMA")
            .await
            .unwrap_or_else(|error| panic!("read pending reviews: {error}")),
        None
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_capture_page_a_cached_schema_extracts_while_schema_review_is_pending() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = open_pending_schema_review(&repository, &id).await;
    let service = manual_scraper(
        schema_service_without_generation(Some(cached_schemas(id, vec![minimal_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    let scraped = result.unwrap_or_else(|error| panic!("expected a scraped product: {error}"));
    assert!(scraped.is_some(), "expected a scraped product");
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap_or_else(|error| panic!("read review pages: {error}"));
    assert!(pages.iter().all(|page| page.role != PAGE_ROLE_WAITING_PAGE));
}

/// A page that fails every cached schema waits on the pending review, attached to it once.
async fn assert_failing_page_waits_on_pending_schema_review(authoring: CrawlerAuthoringMode) {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let waiting_url = Url::parse("https://example.com/products/archived").unwrap();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = open_pending_schema_review(&repository, &id).await;
    let service = scraper_with_review_gate(
        fetcher_returning_sample_page_at(&waiting_url, 2),
        schema_service_without_generation(Some(cached_schemas(id, vec![stale_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
        authoring,
    );

    for _ in 0..2 {
        let result = service
            .scrape(primary_only_request(&id, &waiting_url))
            .await
            .result;
        assert_eq!(pending_review_id(result), review_id);
    }

    let review = repository
        .get_review(review_id)
        .await
        .unwrap_or_else(|error| panic!("read pending review: {error}"));
    let waiting_pages = review
        .pages
        .iter()
        .filter(|page| page.role == PAGE_ROLE_WAITING_PAGE)
        .map(|page| page.url.as_str())
        .collect::<Vec<_>>();
    assert_eq!(waiting_pages, vec![waiting_url.as_str()]);
    assert!(
        review
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_attach_failing_page_to_pending_schema_review_in_manual_mode() {
    assert_failing_page_waits_on_pending_schema_review(CrawlerAuthoringMode::Manual).await;
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_attach_failing_page_to_pending_schema_review_without_generation_in_review_mode() {
    assert_failing_page_waits_on_pending_schema_review(CrawlerAuthoringMode::Review).await;
}
