use super::*;

#[tokio::test]
async fn should_persist_scraped_state_before_marking_url_as_scraped() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let schema = shops_product_schema(id);
    let schema_for_create = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .returning(move |_| {
            let s = vec![schema_for_create.clone()];
            Box::pin(async move { Ok(generated_schemas(s, SchemaLlmEvaluationConfidence::High)) })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save.clone();
            Box::pin(async move { Ok(s) })
        });

    let mut expected = normalized_product(url.clone());
    expected.availability = ListingAvailabilityMapping::Availability(ListingAvailability::SoldOut);
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let url_for_set_presence = url.clone();
    cand_svc
        .expect_set_presence()
        .once()
        .withf(move |received_shop_id, received_url, received_state| {
            *received_shop_id == id
                && received_url == &url_for_set_presence
                && *received_state == UrlPresence::Present
        })
        .returning(|_, _, _| Box::pin(async { Ok(()) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service
        .scrape(&id, &url, None, None)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        result.product.availability,
        ListingAvailabilityMapping::Availability(ListingAvailability::SoldOut)
    );
    assert_eq!(
        result.snapshot.availability.as_deref(),
        Some(ListingAvailability::SoldOut.as_str())
    );
}
