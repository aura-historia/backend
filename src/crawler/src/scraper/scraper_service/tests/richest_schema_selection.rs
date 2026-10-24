use super::*;

/// Helper to build a schema that also has a working description selector so
/// its raw extraction scores higher than a minimal schema.
fn schema_with_description() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.description = Some(ExtractionRule {
        selector: CssSelector::from("main"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });
    schema
}

fn assert_description_captured(
    scraped: &crate::scraper::scraper_service::ScrapedProduct,
    message: &str,
) {
    assert_ne!(
        scraped.raw_input.raw_values().value()["description"]["value"],
        serde_json::json!([]),
        "{message}"
    );
}

#[tokio::test]
async fn should_select_richer_schema_even_when_it_is_later_in_order() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let poor_schema = minimal_schema();
    let rich_schema = schema_with_description();
    // Insert the richer schema at index 1 — first in stored order is the
    // poorer schema, so naive first-success selection would pick it.
    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![poor_schema.clone(), rich_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a cached candidate must be captured"));
    assert_description_captured(&scraped, "richer later schema should win");
}

#[tokio::test]
async fn should_pick_earlier_schema_when_it_extracts_more_data() {
    let id = listing_source_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // Earlier (index 0) extracts description; later (index 1) does not.
    let rich_first = schema_with_description();
    let poor_second = minimal_schema();

    let schema = ListingSourceProductSchema {
        listing_source_id: id,
        product_schemas: vec![rich_first, poor_second],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), CrawlerDisposition::Active);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a cached candidate must be captured"));
    assert_description_captured(&scraped, "earlier richer schema should still win");
}
