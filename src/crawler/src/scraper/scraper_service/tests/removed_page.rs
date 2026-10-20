use super::*;
use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use crate::scraper::css_selector::removed_page_schema_repository::MockRemovedPageSchemaRepository;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::html_fetcher::FetchError;
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

/// The scraper reads the cached schema once per scrape; the removal guard reuses
/// that read.
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

fn normalizing_once(url: &Url) -> MockProductListingNormalizationService {
    let expected = prepared_product(url.clone());
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().once().returning(move |_, _| {
        let product = expected.clone();
        Box::pin(async move { Ok(normalization_success(product, 0)) })
    });
    norm_svc
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
async fn should_mark_product_removed_when_container_level_removed_page_selector_matches() {
    let id = listing_source_id();
    let url = product_url();
    // The stored removed-page selector picks the whole container, so the matched
    // notice is the container text, while the cached schema reads the title from the
    // not-found heading inside it.
    let html = r#"<html><body><main id="mainCatCol">
        <h1>Sorry, the page you're looking for couldn't be found</h1>
        <div class="related"><span id="product-id">SKU-9</span>
        <span id="state">In Stock</span><img src="/images/related.jpg"></div>
        </main></body></html>"#;

    let service = service(
        fetcher_returning(html.to_string()),
        schema_service_with_cached_schema(id),
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed_schema_set_for(
            id,
            "#mainCatCol",
            "couldn't be found",
        )),
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
async fn should_mark_product_removed_when_container_level_removed_page_regex_matches() {
    let id = listing_source_id();
    let url = product_url();
    // Same as the container-level text case, with the notice signalled by a regex.
    let html = r#"<html><body><main id="mainCatCol">
        <h1>Sorry, the page you're looking for couldn't be found</h1>
        <div class="related"><span id="product-id">SKU-9</span>
        <span id="state">In Stock</span><img src="/images/related.jpg"></div>
        </main></body></html>"#;
    let mut removed = removed_schema_set(id);
    removed.removed_page_schemas = vec![RemovedPageSchema {
        selector: CssSelector::from("#mainCatCol"),
        text: None,
        regex: Some(r"couldn't be found".to_string()),
    }];

    let service = service(
        fetcher_returning(html.to_string()),
        schema_service_with_cached_schema(id),
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed),
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
async fn should_mark_product_removed_when_regex_removed_page_schema_matches() {
    let id = listing_source_id();
    let url = product_url();
    let mut removed = removed_schema_set(id);
    removed.removed_page_schemas = vec![RemovedPageSchema {
        selector: CssSelector::from("#mainCatCol h1"),
        text: None,
        regex: Some(r"couldn't be found".to_string()),
    }];

    let service = service(
        fetcher_returning(SOFT_404_HTML.to_string()),
        schema_service_with_cached_schema(id),
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed),
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
async fn should_scrape_product_when_product_image_needs_crawler_image_filtering() {
    let id = listing_source_id();
    let url = product_url();
    // The image attribute carries a control character. The scraper's candidate
    // preparation tolerates it (the URL join strips it or the candidate is dropped);
    // the guard must judge the page the same way instead of treating the candidate
    // as non-product.
    let html = sample_html()
        .replace(
            r#"src="/images/chair-640x640.jpg""#,
            "src=\"/images/chair\t640.jpg\"",
        )
        .replace(
            "<main>",
            r#"<div class="error-template" hidden>Page not found</div><main>"#,
        );

    let service = service(
        fetcher_returning(html),
        schema_service_with_cached_schema(id),
        normalizing_once(&url),
        removed_repo_returning(removed_schema_set_for(
            id,
            ".error-template",
            "Page not found",
        )),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap();

    assert!(scraped.is_some());
}

#[tokio::test]
async fn should_scrape_product_when_removed_page_schema_matches_extractable_product_page() {
    let id = listing_source_id();
    let url = product_url();
    // A site-wide hidden not-found template makes the stored removed-page schema
    // match a live product page that cached schemas still extract. Product evidence
    // wins: the page is scraped and the over-broad template is only logged.
    let html = sample_html().replace(
        "<main>",
        r#"<div class="error-template" hidden>Page not found</div><main>"#,
    );

    let service = service(
        fetcher_returning(html),
        schema_service_with_cached_schema(id),
        normalizing_once(&url),
        removed_repo_returning(removed_schema_set_for(
            id,
            ".error-template",
            "Page not found",
        )),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap();

    assert!(scraped.is_some());
}

#[tokio::test]
async fn should_scrape_product_when_stored_removed_page_regex_matches_any_text() {
    let id = listing_source_id();
    let url = product_url();
    // A stored regex on the title element that matches any title is ignored when
    // applied; it carries no removed-page signal and must not withdraw live products.
    let mut removed = removed_schema_set(id);
    removed.removed_page_schemas = vec![RemovedPageSchema {
        selector: CssSelector::from("h1"),
        text: None,
        regex: Some(".+".to_string()),
    }];

    let service = service(
        fetcher_returning(sample_html()),
        schema_service_with_cached_schema(id),
        normalizing_once(&url),
        removed_repo_returning(removed),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap();

    assert!(scraped.is_some());
}

#[tokio::test]
async fn should_scrape_normal_product_page_when_removed_page_schema_does_not_match() {
    let id = listing_source_id();
    let url = product_url();

    let service = service(
        fetcher_returning(sample_html()),
        schema_service_with_cached_schema(id),
        normalizing_once(&url),
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

#[tokio::test]
async fn should_mark_product_removed_when_cached_schema_only_extracts_a_related_product() {
    let id = listing_source_id();
    let url = product_url();
    // The soft-404 page carries a "you may also like" card that the cached schema
    // reads as a complete product with its own title. Under the listing's known
    // identity that card is not evidence for this listing: the page is removed, not
    // rebound to the related product.
    let html = r#"<html><body><main id="mainCatCol">
        <h1>Sorry, the page you're looking for couldn't be found</h1>
        <div class="related"><h2>Biedermeier Sofa</h2><span id="product-id">SKU-9</span>
        <span id="state">In Stock</span><img src="/images/sofa-640x640.jpg"></div>
        </main></body></html>"#;
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let mut schemas = listing_source_product_schemas(id);
            schemas.product_schemas[0].title.selector = CssSelector::from(".related h2");
            Box::pin(async move { Ok(Some(schemas)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let service = service(
        fetcher_returning(html.to_string()),
        schema_svc,
        MockProductListingNormalizationService::new(),
        removed_repo_returning(removed_schema_set(id)),
    );

    let err = service
        .scrape(ScrapeRequest {
            last_source_listing_id: Some("SKU-42".to_string()),
            ..primary_only_request(&id, &url)
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::ProductListingRemoved { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_not_mark_product_removed_when_cached_candidate_fails_preparation_under_over_broad_template()
 {
    let id = listing_source_id();
    let url = product_url();
    // A live product page matches a hidden site-wide not-found template while its
    // availability text has grown past what deterministic preparation accepts. The
    // cached schema still applies with the product's title and identity, so the
    // page is product evidence: it must reach fresh schema generation, never
    // withdrawal.
    let html = sample_html().replace("In Stock", &"x".repeat(600)).replace(
        "<main>",
        r#"<div class="error-template" hidden>Page not found</div><main>"#,
    );
    let mut schema_svc = schema_service_with_cached_schema(id);
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            let mut schema = minimal_schema();
            schema.title.selector = CssSelector::from("missing-title");
            Box::pin(async move {
                Ok(generated_single_product(
                    schema,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_returning(html)),
        Box::new(schema_svc),
        Box::new(MockProductListingNormalizationService::new()),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
    .with_removed_page_schema_repository(Box::new(removed_repo_returning(
        removed_schema_set_for(id, ".error-template", "Page not found"),
    )));

    let err = service
        .scrape(ScrapeRequest {
            last_source_listing_id: Some("SKU-42".to_string()),
            ..primary_only_request(&id, &url)
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::SchemaRegenerationExhausted { .. }),
        "{err:?}"
    );
}
