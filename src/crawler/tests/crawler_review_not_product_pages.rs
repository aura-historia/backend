//! A review page marked as no product page: the mark, its trail, its exclusion from the schema
//! matrix and the coverage, and the retirement of its URL once the review is approved.

use crawler::review::model::{
    CrawlerReviewEvent, PAGE_ROLE_PRIMARY, ReviewActor, ReviewEventAction, SchemaReviewPageInput,
};
use crawler::review::page_coverage::IncompleteCoverage;
use crawler::review::repository::{Approval, CrawlerReviewRepository, ReviewRepositoryError};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::css_selector::rule::{ExtractionCardinality, ExtractionKind, ExtractionRule};
use crawler::spider::classification::url_metadata::UrlClass;
use crawler::spider::classification::url_metadata_repository::{
    DiscoveredUrl, UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use crawler::{CrawlerReviewId, CrawlerReviewPageId};
use listing_source_core::ListingSourceId;
use serde_json::json;
use sqlx::PgPool;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

const PRODUCT_URL: &str = "https://www.example.com/10001-chair/item";
const PROFILE_URL: &str = "https://www.example.com/10897-simon-wharton-antiques/item";
const PRODUCT_HTML: &str = "<html><body><span class=\"id\">SKU</span><h1>Title</h1><span class=\"state\">In stock</span><img class=\"product\" src=\"a.jpg\"></body></html>";
const PROFILE_HTML: &str = "<html><body><h1>Simon Wharton Antiques</h1></body></html>";

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

/// A ListingSource whose URL pattern takes `/<id>-<slug>/item`, with a pending schema review over
/// a product page and the dealer's profile, which the pattern takes as well and which is a primary
/// page of the review too. Returns the ListingSource, the review and the profile's review page.
async fn review_over_a_product_and_a_profile(
    pool: &PgPool,
) -> (ListingSourceId, CrawlerReviewId, CrawlerReviewPageId) {
    review_of_source_with_crawl_host(pool, "example.com").await
}

async fn review_of_source_with_crawl_host(
    pool: &PgPool,
    crawl_host: &str,
) -> (ListingSourceId, CrawlerReviewId, CrawlerReviewPageId) {
    let listing_source_id = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, \
          crawl_root_url, crawl_host, url_pattern, url_pattern_state, url_pattern_matching, \
          created, updated) \
         VALUES ($1, 'Test source', 'test-source', TRUE, \
                 'https://www.example.com/', $2, '/[0-9]+-[a-z-]+/item$', 'MATCHED', \
                 'FULL_URL_SEARCH', \
                 NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(crawl_host)
    .execute(pool)
    .await
    .unwrap();
    let page = |url: &str, html: &str| SchemaReviewPageInput {
        url: url.to_owned(),
        role: PAGE_ROLE_PRIMARY.to_owned(),
        raw_html: html.to_owned(),
    };
    let repository = CrawlerReviewRepository::new(pool.clone());
    let review_id = repository
        .create_schema_review(
            &listing_source_id,
            "manual_schema_required",
            &[schema()],
            vec![
                page(PRODUCT_URL, PRODUCT_HTML),
                page(PROFILE_URL, PROFILE_HTML),
            ],
            json!({ "manual_authoring": true }),
        )
        .await
        .unwrap();
    let profile = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .find(|page| page.url == PROFILE_URL)
        .unwrap();
    (listing_source_id, review_id, profile.review_page_id)
}

async fn events_of(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
    action: ReviewEventAction,
) -> Vec<CrawlerReviewEvent> {
    repository
        .get_review(review_id)
        .await
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.action == action)
        .collect()
}

/// Evaluates the review's pages as the console does, each with its shop's HTML, and stores the
/// matrix.
async fn store_live_matrix(repository: &CrawlerReviewRepository, review_id: CrawlerReviewId) {
    let pages = repository
        .get_review_pages(review_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| {
            let html = if page.url == PROFILE_URL {
                PROFILE_HTML
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
    repository
        .store_schema_matrix_if_current(review_id, ReviewActor::Human, evaluated)
        .await
        .unwrap();
}

async fn approve(
    repository: &CrawlerReviewRepository,
    review_id: CrawlerReviewId,
) -> Result<(), ReviewRepositoryError> {
    let expected_candidate_version = repository.candidate_version(review_id).await?;
    repository
        .approve_review(
            review_id,
            ReviewActor::Human,
            Approval {
                expected_candidate_version,
                notes: None,
                incomplete_coverage: IncompleteCoverage::Refuse,
            },
        )
        .await
}

async fn insert_product_url(pool: &PgPool, listing_source_id: ListingSourceId, url: &str) {
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, url, url_class, raw_source_record_key, next_retry_at) \
         VALUES ($1, $2, 'product', $2, NOW() + INTERVAL '1 hour')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url)
    .execute(pool)
    .await
    .unwrap();
}

/// The class of the stored URL and whether its retry delay is cleared.
async fn url_state(pool: &PgPool, url: &str) -> (String, bool) {
    sqlx::query_as(
        "SELECT url_class, next_retry_at IS NULL FROM listing_source_urls WHERE url = $1",
    )
    .bind(url)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_mark_a_review_page_as_no_product_and_clear_the_mark_with_an_event_each() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;
    store_live_matrix(&repository, review_id).await;
    let version = repository.candidate_version(review_id).await.unwrap();

    let marked = repository
        .mark_page_not_product(
            review_id,
            profile,
            ReviewActor::AgentAuthor,
            "  the dealer's own profile  ",
        )
        .await
        .unwrap();

    assert_eq!(
        marked.not_product_reason.as_deref(),
        Some("the dealer's own profile")
    );
    let detail = repository.get_review(review_id).await.unwrap();
    assert_eq!(
        detail
            .pages
            .iter()
            .find(|page| page.review_page_id == profile)
            .and_then(|page| page.not_product_reason.as_deref()),
        Some("the dealer's own profile")
    );
    assert_eq!(detail.review.candidate_version, version);
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
    let events = events_of(
        &repository,
        review_id,
        ReviewEventAction::PageMarkedNotProduct,
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, ReviewActor::AgentAuthor);
    assert_eq!(events[0].candidate_version, version);
    assert_eq!(
        events[0].details,
        json!({
            "review_page_id": profile.to_string(),
            "url": PROFILE_URL,
            "reason": "the dealer's own profile",
        })
    );

    store_live_matrix(&repository, review_id).await;
    let cleared = repository
        .clear_page_not_product(review_id, profile, ReviewActor::Human)
        .await
        .unwrap();

    assert_eq!(cleared.not_product_reason, None);
    let detail = repository.get_review(review_id).await.unwrap();
    assert!(
        detail
            .pages
            .iter()
            .all(|page| page.not_product_reason.is_none())
    );
    assert!(
        detail
            .review
            .validation_summary
            .get("schema_matrix")
            .is_none()
    );
    let cleared_events = events_of(
        &repository,
        review_id,
        ReviewEventAction::PageNotProductCleared,
    )
    .await;
    assert_eq!(cleared_events.len(), 1);
    assert_eq!(cleared_events[0].actor, ReviewActor::Human);
    assert!(matches!(
        repository
            .clear_page_not_product(review_id, profile, ReviewActor::Human)
            .await,
        Err(ReviewRepositoryError::ReviewPageNotMarkedNotProduct(id)) if id == profile
    ));
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_a_mark_with_a_blank_or_overlong_reason_or_of_another_review_s_page() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;
    let (_, other_review, _) = review_of_source_with_crawl_host(&pool, "other.example").await;

    for reason in ["", "  \n", &"x".repeat(501)] {
        assert!(matches!(
            repository
                .mark_page_not_product(review_id, profile, ReviewActor::Human, reason)
                .await,
            Err(ReviewRepositoryError::InvalidNotProductReason)
        ));
    }
    assert!(matches!(
        repository
            .mark_page_not_product(other_review, profile, ReviewActor::Human, "profile")
            .await,
        Err(ReviewRepositoryError::ReviewPageNotFound(id)) if id == profile
    ));
    assert!(
        repository
            .mark_page_not_product(review_id, profile, ReviewActor::Human, &"x".repeat(500))
            .await
            .is_ok()
    );
    assert_eq!(
        events_of(
            &repository,
            review_id,
            ReviewEventAction::PageMarkedNotProduct
        )
        .await
        .len(),
        1
    );
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_refuse_the_reviewer_and_a_decided_review_a_mark() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;

    assert!(matches!(
        repository
            .mark_page_not_product(review_id, profile, ReviewActor::AgentReviewer, "profile")
            .await,
        Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates)
    ));
    repository
        .reject_review(review_id, ReviewActor::Human, Some("no product"))
        .await
        .unwrap();
    assert!(matches!(
        repository
            .mark_page_not_product(review_id, profile, ReviewActor::Human, "profile")
            .await,
        Err(ReviewRepositoryError::NotPending(id)) if id == review_id
    ));
    assert!(
        events_of(
            &repository,
            review_id,
            ReviewEventAction::PageMarkedNotProduct
        )
        .await
        .is_empty()
    );
}

/// The profile page is a primary page, so no agent could remove it or acknowledge it away: marked,
/// it needs no coverage and the review is approved with complete coverage of its product page.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_approve_with_complete_coverage_when_the_uncovered_primary_page_is_marked() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let (_, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;
    store_live_matrix(&repository, review_id).await;
    assert!(matches!(
        approve(&repository, review_id).await,
        Err(ReviewRepositoryError::IncompleteCoverage { .. })
    ));

    repository
        .mark_page_not_product(
            review_id,
            profile,
            ReviewActor::AgentAuthor,
            "the dealer's profile",
        )
        .await
        .unwrap();
    store_live_matrix(&repository, review_id).await;

    approve(&repository, review_id).await.unwrap();
    let detail = repository.get_review(review_id).await.unwrap();
    let matrix = detail.review.validation_summary["schema_matrix"]["candidates"][0]["pages"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(matrix, 1);
}

/// Approving the review retires the marked page's URL as a product URL, in every stored spelling
/// and with its retry delay cleared, names it in the approval's event and leaves the product pages.
/// A later crawl that finds the profile linked, which the pattern still takes, keeps it `other`.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_retire_the_marked_url_on_approval_and_keep_it_other_when_a_crawl_finds_it_again() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let url_repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;
    let slashed_profile = format!("{PROFILE_URL}/");
    for url in [PRODUCT_URL, PROFILE_URL, slashed_profile.as_str()] {
        insert_product_url(&pool, listing_source_id, url).await;
    }
    repository
        .mark_page_not_product(
            review_id,
            profile,
            ReviewActor::AgentAuthor,
            "the dealer's profile",
        )
        .await
        .unwrap();
    store_live_matrix(&repository, review_id).await;
    assert_eq!(
        url_state(&pool, PROFILE_URL).await,
        ("product".to_owned(), false)
    );

    approve(&repository, review_id).await.unwrap();

    for url in [PROFILE_URL, slashed_profile.as_str()] {
        assert_eq!(
            url_state(&pool, url).await,
            ("other".to_owned(), true),
            "{url}"
        );
    }
    assert_eq!(
        url_state(&pool, PRODUCT_URL).await,
        ("product".to_owned(), false)
    );
    let approved = events_of(&repository, review_id, ReviewEventAction::Approved).await;
    assert_eq!(
        approved[0].details["retired_not_product_urls"],
        json!([PROFILE_URL])
    );

    let crawled = |raw: &str| DiscoveredUrl {
        url: url::Url::parse(raw).unwrap(),
        url_class: UrlClass::ProductListing,
        canonical: None,
    };
    url_repository
        .upsert_links_batch(
            &listing_source_id,
            &[
                crawled(PROFILE_URL),
                crawled(PRODUCT_URL),
                crawled("https://www.example.com/10002-table/item"),
            ],
        )
        .await
        .unwrap();

    assert_eq!(url_state(&pool, PROFILE_URL).await.0, "other");
    assert_eq!(url_state(&pool, PRODUCT_URL).await.0, "product");
    assert_eq!(
        url_state(&pool, "https://www.example.com/10002-table/item")
            .await
            .0,
        "product"
    );
}

/// A mark of a review that is not approved retires nothing: a crawl keeps classifying the page by
/// the pattern until the review is approved.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_classifying_a_page_by_the_pattern_while_its_mark_is_not_approved() {
    let pool = get_postgres_client().await;
    let repository = CrawlerReviewRepository::new(pool.clone());
    let url_repository = UrlMetadataRepositoryImpl::new(pool.clone());
    let (listing_source_id, review_id, profile) = review_over_a_product_and_a_profile(&pool).await;
    repository
        .mark_page_not_product(
            review_id,
            profile,
            ReviewActor::Human,
            "the dealer's profile",
        )
        .await
        .unwrap();

    url_repository
        .upsert_links_batch(
            &listing_source_id,
            &[DiscoveredUrl {
                url: url::Url::parse(PROFILE_URL).unwrap(),
                url_class: UrlClass::ProductListing,
                canonical: None,
            }],
        )
        .await
        .unwrap();

    assert_eq!(url_state(&pool, PROFILE_URL).await.0, "product");
}
