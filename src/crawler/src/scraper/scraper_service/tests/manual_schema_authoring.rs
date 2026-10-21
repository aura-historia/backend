use super::*;
use crate::authoring::CrawlerAuthoringMode;
use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE};
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use crate::scraper::scraper_service::{ScrapedProduct, ScraperError};
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

fn manual_scraper(
    schema_svc: MockProductListingSchemaService,
    review_repository: CrawlerReviewRepository,
    candidates: MockScraperCandidateService,
) -> ScraperServiceImpl {
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_returning_sample_page()),
        Box::new(schema_svc),
        Box::new(MockProductListingNormalizationService::new()),
        Arc::new(candidates),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_review_gate(review_repository, CrawlerAuthoringMode::Manual)
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
    let mut stale_schema = minimal_schema();
    stale_schema.title.selector = CssSelector::from("h1.no-longer-present");
    let cached = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![stale_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
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
