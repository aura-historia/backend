use super::*;
use crate::scraper::scraper_service::ScrapedProduct;
use crate::scraper::scraper_service::util::hash::{
    EXTRACTOR_VERSION, fingerprint_scraper_context, hash_html, hash_main_fragment,
};

#[tokio::test]
async fn should_skip_when_only_non_main_document_content_changes() {
    let id = listing_source_id();
    let url = product_url();
    let stored_html = sample_html();
    let matching_hash = hash_main_fragment(&stored_html).unwrap_or_else(|| hash_html(&stored_html));
    let fetched_html = stored_html.replacen(
        "<body>",
        r#"<head><meta name="crawler-regression" content="changed"></head><body>"#,
        1,
    );

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = fetched_html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let schema = listing_source_product_schemas(id);
    let schema_fingerprint =
        fingerprint_scraper_context(EXTRACTOR_VERSION, &schema.product_schemas, None)
            .unwrap_or_else(|error| panic!("test schema must serialize: {error}"));
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    let expected_raw_input_sha256 = vec![5; 32];
    let expected_raw_input_sha256_for_mock = expected_raw_input_sha256.clone();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_touch_scraped()
        .once()
        .withf(move |_, _, _, _, actual_raw_input_sha256| {
            actual_raw_input_sha256.as_deref()
                == Some(expected_raw_input_sha256_for_mock.as_slice())
        })
        .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let result = service
        .scrape(ScrapeRequest {
            last_scraped_hash: Some(matching_hash),
            last_scraped_schema_fingerprint: Some(schema_fingerprint),
            expected_last_captured_raw_input_sha256: Some(expected_raw_input_sha256),
            ..primary_only_request(&id, &url)
        })
        .await
        .result
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn should_extract_an_unchanged_page_when_the_extractor_version_was_raised() {
    let id = listing_source_id();
    let previous_version = EXTRACTOR_VERSION - 1;

    let scraped = scrape_unchanged_page(id, scraper_context_fingerprint(previous_version, id), 0)
        .await
        .unwrap_or_else(|| panic!("a raised extractor version must extract the page again"));

    assert_eq!(
        scraped.schema_fingerprint,
        scraper_context_fingerprint(EXTRACTOR_VERSION, id),
        "the scrape records the current extractor version, so the next scrape skips the page"
    );
}

#[tokio::test]
async fn should_skip_an_unchanged_page_when_the_extractor_version_is_unchanged() {
    let id = listing_source_id();

    let scraped =
        scrape_unchanged_page(id, scraper_context_fingerprint(EXTRACTOR_VERSION, id), 1).await;

    assert!(scraped.is_none());
}

fn scraper_context_fingerprint(
    extractor_version: u32,
    listing_source_id: ListingSourceId,
) -> String {
    fingerprint_scraper_context(
        extractor_version,
        &listing_source_product_schemas(listing_source_id).product_schemas,
        None,
    )
    .unwrap_or_else(|error| panic!("test schema must serialize: {error}"))
}

/// Scrapes [`sample_html`] under its cached schema set, both unchanged since the scrape that
/// stored `stored_fingerprint`, and expects the fast path to touch the URL `fast_path_touches`
/// times.
async fn scrape_unchanged_page(
    listing_source_id: ListingSourceId,
    stored_fingerprint: String,
    fast_path_touches: usize,
) -> Option<ScrapedProduct> {
    let url = product_url();
    let stored_html = sample_html();
    let stored_hash = hash_main_fragment(&stored_html).unwrap_or_else(|| hash_html(&stored_html));

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = stored_html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    let schema = listing_source_product_schemas(listing_source_id);
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_touch_scraped()
        .times(fast_path_touches)
        .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    service
        .scrape(ScrapeRequest {
            last_scraped_hash: Some(stored_hash),
            last_scraped_schema_fingerprint: Some(stored_fingerprint),
            ..primary_only_request(&listing_source_id, &url)
        })
        .await
        .result
        .unwrap_or_else(|error| panic!("scraping an unchanged page must succeed: {error}"))
}
