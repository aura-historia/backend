use super::*;
use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use crate::scraper::css_selector::removed_page_schema_repository::MockRemovedPageSchemaRepository;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::FetchError;
use listing_source_core::ListingSourceId;

const SOFT_404_HTML: &str = r#"<html><head><title>Antiques Boutique</title></head><body>
    <main id="mainCatCol"><h1>Sorry, the page you're looking for couldn't be found</h1>
    <img src="/images/logo.png"></main></body></html>"#;

fn removed_schema_set(listing_source_id: ListingSourceId) -> ListingSourceRemovedPageSchema {
    removed_schema_set_for(
        listing_source_id,
        "#mainCatCol h1",
        "Sorry, the page you're looking for couldn't be found",
    )
}

fn removed_schema_set_for(
    listing_source_id: ListingSourceId,
    selector: &str,
    text: &str,
) -> ListingSourceRemovedPageSchema {
    ListingSourceRemovedPageSchema {
        listing_source_id,
        removed_page_schemas: vec![RemovedPageSchema {
            selector: CssSelector::from(selector),
            text: Some(text.to_string()),
            regex: None,
        }],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn fetcher_returning(html: String) -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    fetcher
}

fn removed_repo_returning(row: ListingSourceRemovedPageSchema) -> MockRemovedPageSchemaRepository {
    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .once()
        .returning(move |_| {
            let row = row.clone();
            Box::pin(async move { Ok(Some(row)) })
        });
    removed_repo
}

fn schema_service_with_cached_schema(id: ListingSourceId) -> MockProductListingSchemaService {
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = listing_source_product_schemas(id);
            Box::pin(async move { Ok(Some(schema)) })
        });
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    schema_svc
}

fn service(
    fetcher: MockHtmlFetcher,
    schema_svc: MockProductListingSchemaService,
    norm_svc: MockProductListingNormalizationService,
    removed_repo: MockRemovedPageSchemaRepository,
) -> ScraperServiceImpl {
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(MockScraperCandidateService::new()),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_removed_page_schema_repository(Box::new(removed_repo))
}

#[tokio::test]
async fn should_mark_product_removed_when_stored_removed_page_schema_matches() {
    let id = listing_source_id();
    let url = product_url();

    let service = service(
        fetcher_returning(SOFT_404_HTML.to_string()),
        schema_service_with_cached_schema(id),
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed_schema_set(id)),
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}

#[tokio::test]
async fn should_mark_product_removed_when_removed_page_schema_matches_without_cached_schemas() {
    let id = listing_source_id();
    let url = product_url();

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_generate_single_schema_for_page().never();

    let service = service(
        fetcher_returning(SOFT_404_HTML.to_string()),
        schema_svc,
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed_schema_set(id)),
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}

#[tokio::test]
async fn should_mark_product_removed_when_cached_schema_only_extracts_not_found_notice() {
    let id = listing_source_id();
    let url = product_url();
    // The cached schema reads the not-found heading as the title and the rest from a
    // related-items block; that candidate is the notice, not product evidence.
    let html = r#"<html><body><main id="mainCatCol">
        <h1>Sorry, the page you're looking for couldn't be found</h1>
        <div class="related"><span id="product-id">SKU-9</span>
        <span id="state">In Stock</span><img src="/images/related.jpg"></div>
        </main></body></html>"#;

    let service = service(
        fetcher_returning(html.to_string()),
        schema_service_with_cached_schema(id),
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed_schema_set(id)),
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::ProductListingRemoved { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_preserve_listing_when_removed_page_schema_matches_extractable_product_page() {
    let id = listing_source_id();
    let url = product_url();
    // A site-wide hidden not-found template makes the stored removed-page schema
    // match a live product page that cached schemas still extract.
    let html = sample_html().replace(
        "<main>",
        r#"<div class="error-template" hidden>Page not found</div><main>"#,
    );

    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().never();

    let service = service(
        fetcher_returning(html),
        schema_service_with_cached_schema(id),
        norm_svc,
        removed_repo_returning(removed_schema_set_for(
            id,
            ".error-template",
            "Page not found",
        )),
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::SchemaClassificationRejected { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_preserve_listing_when_broad_removed_page_regex_matches_product_page() {
    let id = listing_source_id();
    let url = product_url();
    let mut removed = removed_schema_set(id);
    removed.removed_page_schemas = vec![RemovedPageSchema {
        selector: CssSelector::from("body"),
        text: None,
        regex: Some(".*".to_string()),
    }];

    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().never();

    let service = service(
        fetcher_returning(sample_html()),
        schema_service_with_cached_schema(id),
        norm_svc,
        removed_repo_returning(removed),
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::SchemaClassificationRejected { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_scrape_normal_product_page_when_removed_page_schema_does_not_match() {
    let id = listing_source_id();
    let url = product_url();

    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().once().returning(move |_, _| {
        let product = expected.clone();
        Box::pin(async move { Ok(normalization_success(product, 0)) })
    });

    let service = service(
        fetcher_returning(sample_html()),
        schema_service_with_cached_schema(id),
        norm_svc,
        removed_repo_returning(removed_schema_set(id)),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap();

    assert!(scraped.is_some());
}

#[tokio::test]
async fn should_not_mark_product_removed_on_transient_fetch_failure() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(|_| {
        Box::pin(async {
            Err(FetchError::Network {
                kind: NetworkErrorKind::HttpStatus(503),
                details: "service unavailable".to_owned(),
            })
        })
    });
    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo.expect_find_removed_page_schema().never();

    let service = service(
        fetcher,
        MockProductListingSchemaService::new(),
        MockProductListingNormalizationService::new(),
        removed_repo,
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(
        !matches!(err, ScraperError::ProductListingRemoved { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_not_mark_product_removed_when_removed_page_schema_lookup_fails() {
    let id = listing_source_id();
    let url = product_url();

    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .once()
        .returning(|_| Box::pin(async { Err(sqlx::Error::PoolTimedOut) }));
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().never();

    let service = service(
        fetcher_returning(SOFT_404_HTML.to_string()),
        schema_svc,
        MockProductListingNormalizationService::new(),
        removed_repo,
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::RemovedPageSchemaDatabaseError(_)),
        "{err:?}"
    );
}
