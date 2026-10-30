use super::*;
use crate::authoring::CrawlerAuthoringMode;
use crate::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crate::review::model::{
    PAGE_ROLE_PRIMARY, PAGE_ROLE_TRIGGERING_GENERATION_PAGE, PAGE_ROLE_WAITING_PAGE, ReviewActor,
    SchemaReviewPageInput,
};
use crate::review::page_coverage::IncompleteCoverage;
use crate::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema_repository::{
    RemovedPageSchemaRepository as _, RemovedPageSchemaRepositoryImpl,
};
use crate::scraper::scraper_service::extraction::manual_schema_review::MANUAL_SCHEMA_REASON;
use crate::scraper::scraper_service::util::hash::{
    EXTRACTOR_VERSION, fingerprint_scraper_context, hash_page,
};
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

/// The candidate version a reviewer reads before editing the review's candidate.
async fn candidate_version(
    repository: &CrawlerReviewRepository,
    review_id: crate::CrawlerReviewId,
) -> i64 {
    repository
        .get_review(review_id)
        .await
        .unwrap_or_else(|error| panic!("read the review: {error}"))
        .review
        .candidate_version
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
        .reject_review(rejected, ReviewActor::Human, Some("not a product page"))
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

/// Rejecting a manual review is a verdict on its triggering page, whichever schemas extract it:
/// once a cached schema extracts the page, such as one another review approved since, the page is
/// still classified as other instead of captured, under the scrape's capture guard, and no review
/// reopens for it. `rescrape` builds the request of the scrape after the rejection from the
/// schemas that extract the page.
async fn assert_rejected_triggering_page_is_not_captured(
    rescrape: impl FnOnce(&ListingSourceId, &Url, &ListingSourceProductSchema) -> ScrapeRequest,
) {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let opening = manual_scraper(
        schema_service_without_generation(Some(cached_schemas(id, vec![stale_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
    );
    let rejected = pending_review_id(opening.scrape(primary_only_request(&id, &url)).await.result);
    let pages = repository
        .get_review_pages(rejected)
        .await
        .unwrap_or_else(|error| panic!("read review pages: {error}"));
    assert_eq!(
        pages
            .iter()
            .map(|page| (page.url.as_str(), page.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(url.as_str(), PAGE_ROLE_TRIGGERING_GENERATION_PAGE)]
    );
    repository
        .reject_review(rejected, ReviewActor::Human, Some("not a product page"))
        .await
        .unwrap_or_else(|error| panic!("reject manual review: {error}"));
    let extracting = cached_schemas(id, vec![minimal_schema()]);
    let request = rescrape(&id, &url, &extracting);
    let mut candidates = MockScraperCandidateService::new();
    let expected_url = url.clone();
    let capture_guard = request.expected_last_captured_raw_input_sha256.clone();
    candidates
        .expect_set_class()
        .once()
        .withf(move |_, received_url, url_class, received_guard| {
            received_url == &expected_url
                && *url_class == crate::spider::classification::url_metadata::UrlClass::Other
                && *received_guard == capture_guard.as_deref()
        })
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    let rescraping = manual_scraper(
        schema_service_without_generation(Some(extracting)),
        repository.clone(),
        candidates,
    );

    let result = rescraping.scrape(request).await.result;

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
async fn should_classify_rejected_triggering_page_as_other_when_a_cached_schema_extracts_it() {
    assert_rejected_triggering_page_is_not_captured(|id, url, _| primary_only_request(id, url))
        .await;
}

/// A rejected page captured before, such as by a cached schema before the rejection took effect,
/// and unchanged since, is classified as other too: the rejection is checked before the
/// unchanged-page skip, under the guard of that capture.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_classify_rejected_triggering_page_as_other_when_it_was_captured_and_is_unchanged() {
    assert_rejected_triggering_page_is_not_captured(|id, url, extracting| ScrapeRequest {
        last_scraped_hash: Some(hash_page(&sample_html(), &extracting.product_schemas)),
        last_scraped_schema_fingerprint: Some(
            fingerprint_scraper_context(EXTRACTOR_VERSION, &extracting.product_schemas, None)
                .unwrap_or_else(|error| panic!("test schemas must serialize: {error}")),
        ),
        expected_last_captured_raw_input_sha256: Some(vec![7; 32]),
        ..primary_only_request(id, url)
    })
    .await;
}

/// The review repository of `agent` authoring mode, whose reviewer agent rejects reviews.
fn agent_deciding_repository(pool: &PgPool) -> CrawlerReviewRepository {
    CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(AgentDecisionPolicy::new(
        CrawlerAuthoringMode::Agent,
        AgentLimits::default(),
    ))
}

/// Rejects the review as the reviewer agent that took its pages for non-product pages, then
/// revokes the rejection as the human who found the mistake.
async fn reject_by_mistake(
    repository: &CrawlerReviewRepository,
    review_id: crate::CrawlerReviewId,
) {
    repository
        .reject_review(
            review_id,
            ReviewActor::AgentReviewer,
            Some("not a product page"),
        )
        .await
        .unwrap_or_else(|error| panic!("reject manual review: {error}"));
    repository
        .revoke_rejection(review_id, ReviewActor::Human, Some("these are products"))
        .await
        .unwrap_or_else(|error| panic!("revoke the rejection: {error}"));
}

/// A scraper whose candidate service fails the test if the scrape classifies its page as other.
fn manual_scraper_keeping_the_page_class(
    schema_svc: MockProductListingSchemaService,
    review_repository: CrawlerReviewRepository,
) -> ScraperServiceImpl {
    let mut candidates = MockScraperCandidateService::new();
    candidates.expect_set_class().never();
    manual_scraper(schema_svc, review_repository, candidates)
}

/// Once a human revokes the rejection, the page is scraped as a product page again: no cached
/// schema extracts it, so it opens a new manual review instead of being classified as other.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_open_a_new_manual_review_for_a_page_once_its_rejection_is_revoked() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = agent_deciding_repository(&pool);
    let opening = manual_scraper(
        schema_service_without_generation(None),
        repository.clone(),
        MockScraperCandidateService::new(),
    );
    let rejected = pending_review_id(opening.scrape(primary_only_request(&id, &url)).await.result);
    reject_by_mistake(&repository, rejected).await;
    let rescraping = manual_scraper_keeping_the_page_class(
        schema_service_without_generation(None),
        repository.clone(),
    );

    let reopened = pending_review_id(
        rescraping
            .scrape(primary_only_request(&id, &url))
            .await
            .result,
    );

    assert_ne!(reopened, rejected);
    let pages = repository
        .get_review_pages(reopened)
        .await
        .unwrap_or_else(|error| panic!("read review pages: {error}"));
    assert_eq!(
        pages
            .iter()
            .map(|page| (page.url.as_str(), page.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(url.as_str(), PAGE_ROLE_PRIMARY)]
    );
}

/// Once a human revokes the rejection of the review a triggering page opened, a cached schema
/// that extracts the page captures it again.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_capture_a_rejected_triggering_page_once_its_rejection_is_revoked() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = agent_deciding_repository(&pool);
    let opening = manual_scraper(
        schema_service_without_generation(Some(cached_schemas(id, vec![stale_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
    );
    let rejected = pending_review_id(opening.scrape(primary_only_request(&id, &url)).await.result);
    reject_by_mistake(&repository, rejected).await;
    let rescraping = manual_scraper_keeping_the_page_class(
        schema_service_without_generation(Some(cached_schemas(id, vec![minimal_schema()]))),
        repository.clone(),
    );

    let result = rescraping
        .scrape(primary_only_request(&id, &url))
        .await
        .result;

    let scraped = result.unwrap_or_else(|error| panic!("expected a scraped product: {error}"));
    assert!(scraped.is_some(), "expected a scraped product");
    assert_eq!(
        repository
            .latest_pending_review_id(&id, "PRODUCT_SCHEMA")
            .await
            .unwrap_or_else(|error| panic!("read pending reviews: {error}")),
        None
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_capture_page_when_a_cached_schema_extracts_it_and_schema_review_is_pending() {
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
async fn should_attach_failing_page_to_pending_schema_review_when_authoring_is_manual() {
    assert_failing_page_waits_on_pending_schema_review(CrawlerAuthoringMode::Manual).await;
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_attach_failing_page_without_generation_when_authoring_is_review() {
    assert_failing_page_waits_on_pending_schema_review(CrawlerAuthoringMode::Review).await;
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_attach_failing_page_without_generation_when_authoring_is_auto() {
    assert_failing_page_waits_on_pending_schema_review(CrawlerAuthoringMode::Auto).await;
}

fn fetcher_returning_once(html: String) -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    fetcher
}

fn missing_required_value(result: Result<Option<ScrapedProduct>, ScraperError>) -> String {
    match result {
        Err(ScraperError::MissingRequiredValue { findings, .. }) => findings.to_string(),
        Err(error) => panic!("expected a missing required value, got {error}"),
        Ok(_) => panic!("expected a missing required value, got a scraped product"),
    }
}

/// A product page that names no item opens no review an operator could only reject, which
/// would classify the product as a non-product page.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_open_no_manual_review_when_the_cached_schema_finds_an_empty_title_element() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let service = scraper_with_review_gate(
        fetcher_returning_once(untitled_html()),
        schema_service_without_generation(Some(cached_schemas(id, vec![item_title_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
        CrawlerAuthoringMode::Manual,
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    assert_eq!(missing_required_value(result), "`title` is blank");
    assert_eq!(
        repository
            .latest_pending_review_id(&id, "PRODUCT_SCHEMA")
            .await
            .unwrap_or_else(|error| panic!("read pending reviews: {error}")),
        None
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_not_wait_on_pending_review_when_the_cached_schema_finds_an_empty_title_element() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = open_pending_schema_review(&repository, &id).await;
    let service = scraper_with_review_gate(
        fetcher_returning_once(untitled_html()),
        schema_service_without_generation(Some(cached_schemas(id, vec![item_title_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
        CrawlerAuthoringMode::Manual,
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    assert_eq!(missing_required_value(result), "`title` is blank");
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap_or_else(|error| panic!("read review pages: {error}"));
    assert!(pages.iter().all(|page| page.role != PAGE_ROLE_WAITING_PAGE));
}

/// An empty title is no reason to skip schema work when the cached schema also captures
/// something other than the product's values from the page.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_open_manual_review_when_an_empty_title_element_comes_with_another_implausible_value()
 {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let page = untitled_html().replace(
        r#"<span id="state">Request for more info</span>"#,
        r#"<span id="state">Sold .badge { color: red; }</span>"#,
    );
    let service = scraper_with_review_gate(
        fetcher_returning_once(page),
        schema_service_without_generation(Some(cached_schemas(id, vec![item_title_schema()]))),
        repository.clone(),
        MockScraperCandidateService::new(),
        CrawlerAuthoringMode::Manual,
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    let review = repository
        .get_review(pending_review_id(result))
        .await
        .unwrap_or_else(|error| panic!("read manual review: {error}"));
    assert_eq!(review.review.reason, MANUAL_SCHEMA_REASON);
}

/// The schema cache serves `stale` on the first read and `approved` on every later one, as when a
/// review is approved while the page is scraped. Generation fails the test.
fn schema_service_approving_during_scrape(
    stale: ListingSourceProductSchema,
    approved: ListingSourceProductSchema,
) -> MockProductListingSchemaService {
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().returning(move |_| {
        let first_read = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let schemas = if first_read {
            stale.clone()
        } else {
            approved.clone()
        };
        Box::pin(async move { Ok(Some(schemas)) })
    });
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    schema_svc
}

/// A page that fails the schemas read before an approval is selected again against the approved
/// schemas, instead of opening a review whose candidate drops them.
async fn assert_page_uses_schemas_approved_during_scrape(authoring: CrawlerAuthoringMode) {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let service = scraper_with_review_gate(
        fetcher_returning_sample_page_at(&url, 1),
        schema_service_approving_during_scrape(
            cached_schemas(id, vec![stale_schema()]),
            cached_schemas(id, vec![stale_schema(), minimal_schema()]),
        ),
        repository.clone(),
        MockScraperCandidateService::new(),
        authoring,
    );

    let result = service.scrape(primary_only_request(&id, &url)).await.result;

    let scraped = result
        .unwrap_or_else(|error| panic!("expected a scraped product: {error}"))
        .unwrap_or_else(|| panic!("expected a scraped product"));
    assert_eq!(
        scraped.raw_input.raw_values().value().get("title"),
        Some(&json!({ "action": "SET", "value": "Biedermeier Chair" }))
    );
    assert_eq!(
        repository
            .latest_pending_review_id(&id, "PRODUCT_SCHEMA")
            .await
            .unwrap_or_else(|error| panic!("read pending reviews: {error}")),
        None
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_use_schemas_approved_during_scrape_when_authoring_is_manual() {
    assert_page_uses_schemas_approved_during_scrape(CrawlerAuthoringMode::Manual).await;
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_use_schemas_approved_during_scrape_when_authoring_is_review() {
    assert_page_uses_schemas_approved_during_scrape(CrawlerAuthoringMode::Review).await;
}

async fn product_schema_review_count(pool: &PgPool, listing_source_id: &ListingSourceId) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM crawler_reviews \
         WHERE listing_source_id = $1 AND artifact_type = 'PRODUCT_SCHEMA'",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("count product-schema reviews: {error}"))
}

/// A needs-repair verdict keeps the review pending: its triggering page and every other failing
/// page wait on it again, and the reviewer's candidate edits survive the next scrapes.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_keep_needs_repair_review_and_its_candidate_edits_when_its_pages_are_scraped_again()
{
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let other_url = Url::parse("https://example.com/products/other-template").unwrap();
    let repository = CrawlerReviewRepository::new(pool.clone());
    let scraper = |page_url: &Url| {
        scraper_with_review_gate(
            fetcher_returning_sample_page_at(page_url, 1),
            schema_service_without_generation(Some(cached_schemas(id, vec![stale_schema()]))),
            repository.clone(),
            MockScraperCandidateService::new(),
            CrawlerAuthoringMode::Manual,
        )
    };
    let review_id = pending_review_id(
        scraper(&url)
            .scrape(primary_only_request(&id, &url))
            .await
            .result,
    );
    let mut edited_schema = minimal_schema();
    edited_schema.title.selector = CssSelector::from("h1.reviewer-edit");
    let edited_candidate = json!({ "schemas": [edited_schema] });
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(&repository, review_id).await,
            edited_candidate.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("edit candidate: {error}"));
    repository
        .request_repair(
            review_id,
            ReviewActor::Human,
            Some("  title selector misses the sold variant  "),
        )
        .await
        .unwrap_or_else(|error| panic!("request repair: {error}"));

    for page_url in [&url, &other_url] {
        let result = scraper(page_url)
            .scrape(primary_only_request(&id, page_url))
            .await
            .result;
        assert_eq!(pending_review_id(result), review_id);
    }

    let review = repository
        .get_review(review_id)
        .await
        .unwrap_or_else(|error| panic!("read review: {error}"));
    assert_eq!(review.review.status, "PENDING_REVIEW");
    assert_eq!(review.review.candidate_payload, edited_candidate);
    assert_eq!(
        review
            .review
            .repair_request
            .map(|request| request.note)
            .as_deref(),
        Some("title selector misses the sold variant")
    );
    let waiting_pages = review
        .pages
        .iter()
        .filter(|page| page.role == PAGE_ROLE_WAITING_PAGE)
        .map(|page| page.url.as_str())
        .collect::<Vec<_>>();
    assert_eq!(waiting_pages, vec![other_url.as_str()]);
    assert_eq!(product_schema_review_count(&pool, &id).await, 1);
}

/// A sold item the shop keeps at HTTP 200 with a notice and no product markup.
const REMOVAL_NOTICE_HTML: &str = r#"<!DOCTYPE html><html><body><main>
    <h1>Biedermeier Chair</h1><p class="notice">This item is no longer available</p>
    </main></body></html>"#;

/// The candidate a reviewer saves to declare the notice page removed: the cached schemas kept
/// as they are, plus a selector-bound template for the notice.
fn candidate_declaring_the_notice_page_removed() -> serde_json::Value {
    json!({
        "schemas": [minimal_schema()],
        "removed_page_schemas": [{ "selector": "p.notice", "text": "no longer available" }],
    })
}

fn manual_scraper_with_removed_page_templates(
    pool: &PgPool,
    html: &'static str,
    cached: ListingSourceProductSchema,
    repository: CrawlerReviewRepository,
) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(move |_| Box::pin(async move { Ok(fetch_result(html.to_owned())) }));
    let pool: &'static PgPool = Box::leak(Box::new(pool.clone()));
    scraper_with_review_gate(
        fetcher,
        schema_service_without_generation(Some(cached)),
        repository,
        MockScraperCandidateService::new(),
        CrawlerAuthoringMode::Manual,
    )
    .with_removed_page_schema_repository(Box::new(RemovedPageSchemaRepositoryImpl::new(pool)))
}

/// Approves the review's current candidate version as `actor`, as an approver who just read it.
async fn approve(
    repository: &CrawlerReviewRepository,
    review_id: crate::CrawlerReviewId,
    actor: ReviewActor,
    notes: Option<&str>,
    incomplete_coverage: IncompleteCoverage,
) -> Result<(), ReviewRepositoryError> {
    let expected_candidate_version = repository.candidate_version(review_id).await?;
    repository
        .approve_review(
            review_id,
            actor,
            Approval {
                expected_candidate_version,
                notes,
                incomplete_coverage,
            },
        )
        .await
}

/// Approves the review the way a reviewer who confirmed its incomplete coverage does: with an
/// acknowledgement of the coverage the refused approval reports.
async fn approve_acknowledging_coverage(
    repository: &CrawlerReviewRepository,
    review_id: crate::CrawlerReviewId,
) -> Result<(), ReviewRepositoryError> {
    match approve(
        repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    {
        Err(ReviewRepositoryError::IncompleteCoverage { coverage, .. }) => {
            approve(
                repository,
                review_id,
                ReviewActor::Human,
                None,
                IncompleteCoverage::Acknowledged(coverage),
            )
            .await
        }
        decided => decided,
    }
}

/// Opens the manual review for the notice page, declares the page removed on it, evaluates the
/// candidate on the page as a matrix refresh does and approves. The stored matrix shows the
/// template captures the review's only page as removed, so the approval needs no acknowledgement
/// of incomplete coverage.
async fn approve_removed_page_template_for_notice_page(
    pool: &PgPool,
    id: &ListingSourceId,
    repository: &CrawlerReviewRepository,
) -> crate::CrawlerReviewId {
    let opening = manual_scraper_with_removed_page_templates(
        pool,
        REMOVAL_NOTICE_HTML,
        cached_schemas(*id, vec![minimal_schema()]),
        repository.clone(),
    );
    let review_id = pending_review_id(
        opening
            .scrape(primary_only_request(id, &product_url()))
            .await
            .result,
    );
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            candidate_version(repository, review_id).await,
            candidate_declaring_the_notice_page_removed(),
        )
        .await
        .unwrap_or_else(|error| panic!("declare the page removed: {error}"));
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap_or_else(|error| panic!("read the review pages: {error}"))
        .into_iter()
        .map(|page| (page, REMOVAL_NOTICE_HTML.to_owned()))
        .collect();
    let evaluated = repository
        .evaluate_schema_matrix_for_live_pages(review_id, pages, time::OffsetDateTime::now_utc())
        .await
        .unwrap_or_else(|error| panic!("evaluate the declaration: {error}"));
    repository
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
        .await
        .unwrap_or_else(|error| panic!("store the matrix: {error}"));
    approve(
        repository,
        review_id,
        ReviewActor::Human,
        None,
        IncompleteCoverage::Refuse,
    )
    .await
    .unwrap_or_else(|error| panic!("approve the removed-page template: {error}"));
    review_id
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_capture_a_notice_page_as_removed_once_its_review_approves_a_template() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    approve_removed_page_template_for_notice_page(&pool, &id, &repository).await;
    let rescraping = manual_scraper_with_removed_page_templates(
        &pool,
        REMOVAL_NOTICE_HTML,
        cached_schemas(id, vec![minimal_schema()]),
        repository.clone(),
    );

    let result = rescraping
        .scrape(primary_only_request(&id, &product_url()))
        .await
        .result;

    assert!(
        matches!(result, Err(ScraperError::ProductListingRemoved { .. })),
        "expected the verified removal that becomes the raw delete capture, got {result:?}"
    );
    assert_eq!(
        repository
            .latest_pending_review_id(&id, "PRODUCT_SCHEMA")
            .await
            .unwrap_or_else(|error| panic!("read pending reviews: {error}")),
        None,
        "the notice page must open no second review"
    );
    let stored = RemovedPageSchemaRepositoryImpl::new(&pool)
        .find_removed_page_schema(&id)
        .await
        .unwrap_or_else(|error| panic!("read removed-page templates: {error}"))
        .map(|row| row.removed_page_schemas);
    assert_eq!(
        stored,
        Some(vec![RemovedPageSchema {
            selector: CssSelector::from("p.notice"),
            text: Some("no longer available".to_owned()),
            regex: None,
        }])
    );
}

/// A rejection is a verdict that its pages are no product pages, not that their listings stay
/// live: a rejected page that a stored removed-page template matches is still captured as removed,
/// so the raw delete capture withdraws the listing a capture before the rejection made visible.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_capture_a_rejected_page_as_removed_when_a_removed_page_template_matches_it() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    approve_removed_page_template_for_notice_page(&pool, &id, &repository).await;
    let rejected = repository
        .create_schema_review(
            &id,
            MANUAL_SCHEMA_REASON,
            &[minimal_schema()],
            vec![SchemaReviewPageInput {
                url: url.to_string(),
                role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_owned(),
                raw_html: sample_html(),
            }],
            json!({}),
        )
        .await
        .unwrap_or_else(|error| panic!("open manual review: {error}"));
    repository
        .reject_review(rejected, ReviewActor::Human, Some("not a product page"))
        .await
        .unwrap_or_else(|error| panic!("reject manual review: {error}"));
    assert!(
        repository
            .has_rejected_schema_review_for_page(&id, MANUAL_SCHEMA_REASON, url.as_str())
            .await
            .unwrap_or_else(|error| panic!("read rejected pages: {error}")),
        "the page must be a rejected page"
    );

    let result = scrape_notice_page(&pool, &id, &repository, REMOVAL_NOTICE_HTML).await;

    assert!(
        matches!(result, Err(ScraperError::ProductListingRemoved { .. })),
        "the verified removal must win over the rejection, got {result:?}"
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_scrape_a_product_page_the_approved_template_matches_while_a_cached_schema_extracts_it()
 {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    approve_removed_page_template_for_notice_page(&pool, &id, &repository).await;
    let product_with_a_sold_related_item: &'static str = Box::leak(
        sample_html()
            .replace(
                "</main>",
                r#"<aside><p class="notice">This item is no longer available</p></aside></main>"#,
            )
            .into_boxed_str(),
    );
    let service = manual_scraper_with_removed_page_templates(
        &pool,
        product_with_a_sold_related_item,
        cached_schemas(id, vec![minimal_schema()]),
        repository,
    );

    let result = service
        .scrape(primary_only_request(&id, &product_url()))
        .await
        .result;

    let scraped = result.unwrap_or_else(|error| panic!("expected a scraped product: {error}"));
    assert!(
        scraped.is_some(),
        "the cached schema's product must win over the template"
    );
}

/// Opens a review on a ListingSource whose notice page is captured as removed, saves
/// `removed_page_schemas` in place of the stored templates the candidate opened with, and
/// approves.
async fn approve_reviewed_removed_page_templates(
    id: &ListingSourceId,
    repository: &CrawlerReviewRepository,
    removed_page_schemas: serde_json::Value,
) {
    let review_id = open_pending_schema_review(repository, id).await;
    let opened = repository
        .get_review(review_id)
        .await
        .unwrap_or_else(|error| panic!("read the review: {error}"));
    assert_eq!(
        opened.review.candidate_payload["removed_page_schemas"],
        candidate_declaring_the_notice_page_removed()["removed_page_schemas"],
        "the review must open with the stored template"
    );
    repository
        .update_candidate_payload(
            review_id,
            ReviewActor::Human,
            opened.review.candidate_version,
            json!({
                "schemas": [minimal_schema()],
                "removed_page_schemas": removed_page_schemas,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("edit the stored templates: {error}"));
    approve_acknowledging_coverage(repository, review_id)
        .await
        .unwrap_or_else(|error| panic!("approve the edited templates: {error}"));
}

async fn scrape_notice_page(
    pool: &PgPool,
    id: &ListingSourceId,
    repository: &CrawlerReviewRepository,
    html: &'static str,
) -> Result<Option<ScrapedProduct>, ScraperError> {
    manual_scraper_with_removed_page_templates(
        pool,
        html,
        cached_schemas(*id, vec![minimal_schema()]),
        repository.clone(),
    )
    .scrape(primary_only_request(id, &product_url()))
    .await
    .result
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_stop_capturing_a_notice_page_as_removed_once_a_review_retires_its_template() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    approve_removed_page_template_for_notice_page(&pool, &id, &repository).await;

    approve_reviewed_removed_page_templates(&id, &repository, json!([])).await;
    let result = scrape_notice_page(&pool, &id, &repository, REMOVAL_NOTICE_HTML).await;

    assert!(
        matches!(result, Err(ScraperError::PendingSchemaReview { .. })),
        "a page no template matches must wait on a schema review, got {result:?}"
    );
}

#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_match_what_the_edited_template_declares_once_a_review_edits_it() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    approve_removed_page_template_for_notice_page(&pool, &id, &repository).await;
    const WITHDRAWN_NOTICE_HTML: &str = r#"<!DOCTYPE html><html><body><main>
    <h1>Biedermeier Chair</h1><p class="notice">This item was withdrawn from sale</p>
    </main></body></html>"#;

    approve_reviewed_removed_page_templates(
        &id,
        &repository,
        json!([{ "selector": "p.notice", "text": "withdrawn from sale" }]),
    )
    .await;
    let withdrawn = scrape_notice_page(&pool, &id, &repository, WITHDRAWN_NOTICE_HTML).await;
    let no_longer_available =
        scrape_notice_page(&pool, &id, &repository, REMOVAL_NOTICE_HTML).await;

    assert!(
        matches!(withdrawn, Err(ScraperError::ProductListingRemoved { .. })),
        "the edited template must capture its notice page as removed, got {withdrawn:?}"
    );
    assert!(
        matches!(
            no_longer_available,
            Err(ScraperError::PendingSchemaReview { .. })
        ),
        "the notice the template no longer declares must not be captured as removed, got {no_longer_available:?}"
    );
}

/// Stores `urls` as the active product URLs of the ListingSource's domain `example.com`, in the
/// spellings given, and returns that domain.
async fn store_product_urls(
    pool: &PgPool,
    listing_source_id: &ListingSourceId,
    urls: &[&str],
) -> crate::CrawlerDomainId {
    let domain_id = crate::CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains \
         (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'example.com', 'example.com')",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert domain: {error}"));
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, raw_source_record_key, url_class) \
         SELECT $1, $2, url, url, 'product' FROM UNNEST($3::text[]) AS url",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .bind(urls)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("store product URLs: {error}"));
    domain_id
}

/// Opens a manual review over `pages`, recorded as `(url, role)`, and rejects it as an operator
/// who found that they are no product pages.
async fn reject_manual_review_over(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
    pages: &[(&str, &str)],
) {
    let review_id = repository
        .create_schema_review(
            listing_source_id,
            MANUAL_SCHEMA_REASON,
            &[minimal_schema()],
            pages
                .iter()
                .map(|(url, role)| SchemaReviewPageInput {
                    url: (*url).to_owned(),
                    role: (*role).to_owned(),
                    raw_html: sample_html(),
                })
                .collect(),
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap_or_else(|error| panic!("open manual review: {error}"));
    repository
        .reject_review(review_id, ReviewActor::Human, Some("not product pages"))
        .await
        .unwrap_or_else(|error| panic!("reject manual review: {error}"));
}

/// A manual-mode scraper without cached schemas that seeds a new review with up to three pages
/// it samples from the stored product URLs. It serves the sample product page from every URL and
/// fails the test unless it fetches exactly `fetches` pages, the scraped page included.
fn seeding_manual_scraper(
    pool: &PgPool,
    repository: CrawlerReviewRepository,
    fetches: usize,
) -> ScraperServiceImpl {
    seeding_manual_scraper_over(
        Arc::new(crate::scraper::candidate_service::ScraperCandidateServiceImpl::new(pool.clone())),
        repository,
        fetches,
    )
}

/// [`seeding_manual_scraper`] sampling seed pages from `candidates`.
fn seeding_manual_scraper_over(
    candidates: Arc<dyn crate::scraper::candidate_service::ScraperCandidateService>,
    repository: CrawlerReviewRepository,
    fetches: usize,
) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().times(fetches).returning(|url| {
        let url = url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), url)) })
    });
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_service_without_generation(None)),
        candidates,
        4,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_review_gate(repository, CrawlerAuthoringMode::Manual)
}

/// Scrapes the product page `url` of the domain `domain_id` until it opens a manual review, and
/// returns the URLs of that review's pages, sorted.
async fn new_manual_review_page_urls(
    scraper: &ScraperServiceImpl,
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
    domain_id: crate::CrawlerDomainId,
    url: &Url,
) -> Vec<String> {
    let request = ScrapeRequest {
        domain_id: Some(domain_id),
        ..normal_request(listing_source_id, url)
    };
    let review_id = pending_review_id(scraper.scrape(request).await.result);
    let mut urls: Vec<String> = repository
        .get_review_pages(review_id)
        .await
        .unwrap_or_else(|error| panic!("read review pages: {error}"))
        .into_iter()
        .map(|page| page.url)
        .collect();
    urls.sort();
    urls
}

const REJECTED_PRIMARY_RECORDED: &str = "https://example.com/products/rejected-primary";
const REJECTED_PRIMARY_STORED: &str = "https://example.com/products/rejected-primary/";
const REJECTED_SEED_RECORDED: &str = "https://example.com/products/rejected-seed/";
const REJECTED_SEED_STORED: &str = "https://example.com/products/rejected-seed";

/// Rejects a manual review over two pages the spider has stored since under the other
/// trailing-slash spelling than the review recorded.
async fn reject_two_stored_pages(
    repository: &CrawlerReviewRepository,
    listing_source_id: &ListingSourceId,
) {
    reject_manual_review_over(
        repository,
        listing_source_id,
        &[
            (REJECTED_PRIMARY_RECORDED, PAGE_ROLE_PRIMARY),
            (REJECTED_SEED_RECORDED, crate::review::model::PAGE_ROLE_SEED),
        ],
    )
    .await;
}

/// Rejecting a manual review classifies its pages as non-product pages, so the ListingSource's next
/// review is not judged on them: its seed sample leaves them out in either trailing-slash spelling
/// and takes the domain's other product pages instead, without fetching a rejected one.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_seed_a_new_manual_review_with_other_pages_than_the_rejected_ones() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let other_pages = [
        "https://example.com/products/other-1",
        "https://example.com/products/other-2",
    ];
    let domain_id = store_product_urls(
        &pool,
        &id,
        &[
            url.as_str(),
            REJECTED_PRIMARY_STORED,
            REJECTED_SEED_STORED,
            other_pages[0],
            other_pages[1],
        ],
    )
    .await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    reject_two_stored_pages(&repository, &id).await;
    let scraper = seeding_manual_scraper(&pool, repository.clone(), 3);

    let pages = new_manual_review_page_urls(&scraper, &repository, &id, domain_id, &url).await;

    assert_eq!(pages, vec![url.as_str(), other_pages[0], other_pages[1]]);
}

/// A rejected page never fills a seed sample: when rejections cover every other product page of
/// the domain, the new review opens with the page that opened it alone.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_open_a_new_manual_review_with_its_page_alone_when_every_other_page_is_rejected() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let domain_id = store_product_urls(
        &pool,
        &id,
        &[url.as_str(), REJECTED_PRIMARY_STORED, REJECTED_SEED_STORED],
    )
    .await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    reject_two_stored_pages(&repository, &id).await;
    let scraper = seeding_manual_scraper(&pool, repository.clone(), 1);

    let pages = new_manual_review_page_urls(&scraper, &repository, &id, domain_id, &url).await;

    assert_eq!(pages, vec![url.as_str()]);
}

/// Expects one schema-seed draw of `limit` URLs that leaves out exactly `excluded`, answered with
/// `drawn`.
fn expect_seed_draw(
    candidates: &mut MockScraperCandidateService,
    excluded: &[&Url],
    limit: i64,
    drawn: &[&Url],
) {
    let excluded: Vec<String> = excluded.iter().map(ToString::to_string).collect();
    let drawn: Vec<crate::scraper::candidate_service::SchemaSeedCandidate> = drawn
        .iter()
        .map(
            |url| crate::scraper::candidate_service::SchemaSeedCandidate {
                url: (*url).clone(),
                expected_last_captured_raw_input_sha256: None,
            },
        )
        .collect();
    candidates
        .expect_get_random_product_urls_for_schema_seed()
        .once()
        .withf(move |_, _, received, received_limit| {
            received == excluded.as_slice() && *received_limit == limit
        })
        .returning(move |_, _, _, _| {
            let drawn = drawn.clone();
            Box::pin(async move { Ok(drawn) })
        });
}

/// A rejected page drawn as a seed is replaced by drawing again for the pages still missing,
/// without every page drawn before, rejected or not. The sample query matches a page by its
/// stored spelling, so a page stored in another spelling than the URL it parses to can come back:
/// it is not taken twice, and once a draw brings no page not drawn before, sampling stops instead
/// of drawing a rejected page forever.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_draw_again_for_rejected_seed_pages_until_a_draw_brings_only_pages_drawn_before() {
    let pool = test_api::get_postgres_client().await;
    let id = insert_listing_source(&pool).await;
    let url = product_url();
    let repository = CrawlerReviewRepository::new(pool.clone());
    reject_two_stored_pages(&repository, &id).await;
    let parse = |url: &str| Url::parse(url).unwrap_or_else(|error| panic!("{url}: {error}"));
    let rejected_seed = parse(REJECTED_SEED_STORED);
    let rejected_primary = parse(REJECTED_PRIMARY_STORED);
    let others = [
        parse("https://example.com/products/other-1"),
        parse("https://example.com/products/other-2"),
    ];
    let mut candidates = MockScraperCandidateService::new();
    expect_seed_draw(
        &mut candidates,
        &[&url],
        3,
        &[&rejected_seed, &others[0], &rejected_primary],
    );
    expect_seed_draw(
        &mut candidates,
        &[&url, &rejected_seed, &others[0], &rejected_primary],
        2,
        &[&others[0], &others[1]],
    );
    expect_seed_draw(
        &mut candidates,
        &[
            &url,
            &rejected_seed,
            &others[0],
            &rejected_primary,
            &others[1],
        ],
        1,
        &[&rejected_seed],
    );
    let scraper = seeding_manual_scraper_over(Arc::new(candidates), repository.clone(), 3);

    let pages = new_manual_review_page_urls(
        &scraper,
        &repository,
        &id,
        crate::CrawlerDomainId::new(),
        &url,
    )
    .await;

    assert_eq!(
        pages,
        vec![url.as_str(), others[0].as_str(), others[1].as_str()]
    );
}
