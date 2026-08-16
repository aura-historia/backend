use super::*;
use crate::network::policy::NetworkErrorKind;
use crate::scraper::css_selector::currency_dto::CurrencyDto;
use crate::scraper::scraper_service::service::FetchError;

#[tokio::test]
async fn should_seed_schema_generation_with_additional_sample_pages_on_cache_miss() {
    let id = shop_id();
    let url = product_url();
    let primary_html = sample_html();

    let mut fetcher = MockHtmlFetcher::new();
    let primary_html_for_fetch = primary_html.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = primary_html_for_fetch.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let initial_schema = {
        let text_rule = |selector: &str| ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        };
        let attr_rule_all = |selector: &str, attr: &str| ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: attr.into() },
            cardinality: ExtractionCardinality::All,
        };
        ProductCssSelectorSchema {
            shops_product_id: Some(text_rule("non-existent-id")),
            title: text_rule("non-existent-title"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: text_rule("non-existent-state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: CurrencyDto::Eur,
            raw_attributes: Default::default(),
        }
    };

    let schema = shops_product_schema(id);
    let final_schema_for_append = schema.product_schemas.first().cloned().unwrap();

    let mut schema_svc = MockProductSchemaService::new();
    let initial_schema_for_find = initial_schema.clone();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = ShopsProductSchema {
                shop_id: shop_id(),
                product_schemas: vec![initial_schema_for_find.clone()],
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_append_single_schema()
        .once()
        .returning(move |_| {
            let s = final_schema_for_append.clone();
            Box::pin(async move {
                Ok(generated_append_product(
                    s,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    let schema_for_persist = schema.clone();
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_persist.clone();
            Box::pin(async move { Ok(s) })
        });

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service
        .scrape(&id, &url, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.product.shops_product_id,
        ShopsProductId::from("SKU-42")
    );
}

#[tokio::test]
async fn should_fallback_to_primary_page_when_schema_seed_sampling_query_fails() {
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
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
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

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    cand_svc
        .expect_get_random_product_urls_for_schema_seed()
        .once()
        .returning(|_, _, _| Box::pin(async { Err(sqlx::Error::RowNotFound) }));
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service
        .scrape(&id, &url, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.product.state, ProductState::Available);
}

#[tokio::test]
async fn should_keep_primary_only_when_extra_schema_seed_fetch_fails() {
    let id = shop_id();
    let url = product_url();
    let sample_seed_url = Url::parse("https://example.com/product/seed-fail").unwrap();
    let expected_primary_url = url.clone();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .times(2)
        .returning(move |requested_url| {
            let requested_url = requested_url.clone();
            let expected_primary_url = expected_primary_url.clone();
            Box::pin(async move {
                if requested_url == expected_primary_url {
                    Ok(fetch_result(sample_html()))
                } else {
                    Err(FetchError::Network {
                        kind: NetworkErrorKind::Timeout,
                        details: "timeout".to_string(),
                    })
                }
            })
        });

    let schema = shops_product_schema(id);
    let schema_for_create = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
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

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let sample_seed_url_clone = sample_seed_url.clone();
    cand_svc
        .expect_get_random_product_urls_for_schema_seed()
        .once()
        .returning(move |_, _, _| {
            let sampled = vec![sample_seed_url_clone.clone()];
            Box::pin(async move { Ok(sampled) })
        });
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service
        .scrape(&id, &url, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.product.state, ProductState::Available);
}

#[tokio::test]
async fn should_skip_schema_seed_page_when_redirected_url_does_not_match_product_pattern() {
    let id = shop_id();
    let url = product_url();
    let sample_seed_url = Url::parse("https://example.com/products/seed").unwrap();
    let redirected_category_url = Url::parse("https://example.com/collections/chairs").unwrap();
    let expected_primary_url = url.clone();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .times(2)
        .returning(move |requested_url| {
            let requested_url = requested_url.clone();
            let expected_primary_url = expected_primary_url.clone();
            let redirected_category_url = redirected_category_url.clone();
            Box::pin(async move {
                if requested_url == expected_primary_url {
                    Ok(fetch_result(sample_html()))
                } else {
                    Ok(fetch_result_for(sample_html(), redirected_category_url))
                }
            })
        });

    let schema = shops_product_schema(id);
    let schema_for_create = schema.product_schemas.first().cloned().unwrap();
    let schema_for_save = schema.clone();
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
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

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let sample_seed_url_clone = sample_seed_url.clone();
    cand_svc
        .expect_get_random_product_urls_for_schema_seed()
        .once()
        .returning(move |_, _, _| {
            let sampled = vec![sample_seed_url_clone.clone()];
            Box::pin(async move { Ok(sampled) })
        });
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service
        .scrape(&id, &url, Some(r"/products/"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.product.state, ProductState::Available);
}

#[tokio::test]
async fn should_not_query_seed_urls_when_schema_seed_pages_is_one() {
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
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc
        .expect_create_product_schemas()
        .once()
        .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
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

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(normalization_success(n, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap().is_some());
}
