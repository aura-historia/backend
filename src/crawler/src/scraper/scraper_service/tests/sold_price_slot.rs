//! A cached schema whose price rule reads the status a sold page prints in the price's place
//! (#2879): the status lowers the candidate's rank but does not reject it, so the page is still
//! captured, as sold, and keeps the listing's last price.

use super::*;

fn price_slot_schema(listing_source_id: ListingSourceId) -> ListingSourceProductSchema {
    let mut schemas = listing_source_product_schemas(listing_source_id);
    schemas.product_schemas[0].price = Some(ExtractionRule {
        selector: CssSelector::from("div.price"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });
    schemas
}

/// Scrapes a sold page that shows `VERKAUFT €` where the price was and has no `#state`.
async fn scrape_sold_page() -> crate::scraper::scraper_service::ScrapedProduct {
    let id = listing_source_id();
    let url = product_url();
    let html = r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <div class="price"><span>VERKAUFT<span class="price_post">&nbsp;€</span></span></div>
        <img src="/images/chair-640x640.jpg">
      </main>
    </body>
    </html>"#
        .to_owned();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    let schema = price_slot_schema(id);
    let mut schema_service = MockProductListingSchemaService::new();
    schema_service
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_service),
        Arc::new(MockScraperCandidateService::new()),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    service
        .scrape(primary_only_request(&id, &url))
        .await
        .result
        .unwrap_or_else(|error| panic!("sold page scrape must succeed: {error}"))
        .unwrap_or_else(|| panic!("sold page scrape must produce a raw observation"))
}

#[tokio::test]
async fn should_capture_a_sold_price_slot_as_sold_and_keep_the_listing_price() {
    let product = scrape_sold_page().await;
    let raw_values = product.raw_input.raw_values().value();

    assert_eq!(
        raw_values.get("price"),
        Some(&serde_json::json!({"action": "UNCHANGED"}))
    );
    assert_eq!(
        raw_values.get("availability"),
        Some(&serde_json::json!({"action": "SET", "value": "VERKAUFT"}))
    );
    assert_eq!(
        product.raw_input.source_payload().value().get("price"),
        Some(&serde_json::json!("VERKAUFT\u{a0}€"))
    );
}
