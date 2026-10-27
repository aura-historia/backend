use super::*;
use crate::scraper::scraper_service::ScrapedProduct;
use crate::scraper::scraper_service::util::hash::{
    EXTRACTOR_VERSION, fingerprint_scraper_context, hash_page,
};

/// [`sample_html`] with the per-request tokens shops render inside `<main>`: a checkout link's
/// session id (Harrison Antique Furniture), and an enquiry form's anti-spam tokens and honeypot
/// label, whose words WPForms shuffles per request (Collinge Antiques).
fn sample_html_with_request_tokens(session_id: &str, form_token: &str, label: &str) -> String {
    sample_html().replace(
        "</main>",
        &format!(
            r#"<span class="checkout"><a href="/index.asp?function=CHECKOUT&uid={session_id}">Checkout</a></span>
            <form data-token-time="{form_token}">
              <label for="wpforms-37-field_4">{label}</label>
              <input type="text" id="wpforms-37-field_4" name="wpforms[fields][4]">
              <input type="hidden" name="wpforms[time_token]" value="{form_token}">
            </form>
          </main>"#
        ),
    )
}

#[tokio::test]
async fn should_skip_a_page_that_differs_only_in_per_request_tokens() {
    let id = listing_source_id();
    let url = product_url();
    let stored_html =
        sample_html_with_request_tokens("3C264776", "1793103358", "Subject Name Message");
    let fetched_html =
        sample_html_with_request_tokens("B08072A2", "1793103362", "Address Name Subject");
    let matching_hash = hash_page(
        &stored_html,
        &listing_source_product_schemas(id).product_schemas,
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

    let scraped = scrape_page(
        id,
        &sample_html(),
        sample_html(),
        scraper_context_fingerprint(previous_version, id),
        0,
    )
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

    let scraped = scrape_page(
        id,
        &sample_html(),
        sample_html(),
        scraper_context_fingerprint(EXTRACTOR_VERSION, id),
        1,
    )
    .await;

    assert!(scraped.is_none());
}

#[tokio::test]
async fn should_extract_a_page_whose_product_values_changed() {
    let id = listing_source_id();
    let sold_html = sample_html().replace("In Stock", "Sold");

    let scraped = scrape_page(
        id,
        &sample_html(),
        sold_html.clone(),
        scraper_context_fingerprint(EXTRACTOR_VERSION, id),
        0,
    )
    .await
    .unwrap_or_else(|| panic!("a changed product value must extract the page"));

    assert_eq!(
        scraped.hash,
        hash_page(
            &sold_html,
            &listing_source_product_schemas(id).product_schemas
        ),
        "the scrape records the changed page's hash, so the next scrape skips the page"
    );
}

#[tokio::test]
async fn should_skip_an_unchanged_page_without_a_main_element() {
    let id = listing_source_id();
    let html = sample_html()
        .replace("<main>", "<div>")
        .replace("</main>", "</div>");

    let scraped = scrape_page(
        id,
        &html,
        html.clone(),
        scraper_context_fingerprint(EXTRACTOR_VERSION, id),
        1,
    )
    .await;

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

/// Scrapes `fetched_html` under the cached schema set, after a scrape that stored the hash of
/// `stored_html` and `stored_fingerprint`, and expects the fast path to touch the URL
/// `fast_path_touches` times.
async fn scrape_page(
    listing_source_id: ListingSourceId,
    stored_html: &str,
    fetched_html: String,
    stored_fingerprint: String,
    fast_path_touches: usize,
) -> Option<ScrapedProduct> {
    let url = product_url();
    let stored_hash = hash_page(
        stored_html,
        &listing_source_product_schemas(listing_source_id).product_schemas,
    );

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = fetched_html.clone();
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
        .unwrap_or_else(|error| panic!("scraping the page must succeed: {error}"))
}
