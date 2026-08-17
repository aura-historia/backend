use super::*;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::util::html::is_schema_specific_normalization_error;

#[test]
fn should_not_classify_no_valid_images_as_schema_specific() {
    assert!(!is_schema_specific_normalization_error(
        &NormalizationError::NoValidImages { candidates: 2 }
    ));
}

#[test]
fn should_classify_title_errors_as_schema_specific() {
    assert!(is_schema_specific_normalization_error(
        &NormalizationError::TitleEmpty
    ));
}

#[tokio::test]
async fn should_try_next_existing_schema_when_first_schema_has_fixable_normalization_error() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let first_schema = minimal_schema();
    let second_schema = minimal_schema();
    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![first_schema, second_schema],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let expected = normalized_product(url.clone());
    let norm_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(2)
        .returning(move |_, _, _| {
            let n = expected.clone();
            let norm_calls = norm_calls.clone();
            Box::pin(async move {
                if norm_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err(normalization_failure(NormalizationError::TitleEmpty, 0))
                } else {
                    Ok(normalization_success(n, 0))
                }
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

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
        result.product.shops_product_id,
        ShopsProductId::from("SKU-42")
    );
}

#[tokio::test]
async fn should_try_all_existing_schemas_before_repairing_fixable_normalization_error() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let first_schema = minimal_schema();
    let mut second_schema = minimal_schema();
    second_schema.description = Some(ExtractionRule {
        selector: CssSelector::from("main"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });

    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![first_schema, second_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            Box::pin(async {
                Ok(generated_append_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            let saved = ShopsProductSchema {
                shop_id: id,
                product_schemas: schemas,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            Box::pin(async move { Ok(saved) })
        });

    let expected = normalized_product(url.clone());
    let norm_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(3)
        .returning(move |_, _, _| {
            let n = expected.clone();
            let norm_calls = norm_calls.clone();
            Box::pin(async move {
                if norm_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                    Err(normalization_failure(NormalizationError::TitleEmpty, 0))
                } else {
                    Ok(normalization_success(n, 0))
                }
            })
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
async fn should_regenerate_schema_when_normalization_error_is_fixable() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let existing_schema = minimal_schema();
    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![existing_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let s = minimal_schema();
            Box::pin(async move {
                Ok(generated_append_product(
                    s,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            let saved = ShopsProductSchema {
                shop_id: id,
                product_schemas: schemas,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            Box::pin(async move { Ok(saved) })
        });

    let expected = normalized_product(url.clone());
    let norm_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(2)
        .returning(move |_, _, _| {
            let n = expected.clone();
            let norm_calls = norm_calls.clone();
            Box::pin(async move {
                if norm_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err(normalization_failure(NormalizationError::TitleEmpty, 0))
                } else {
                    Ok(normalization_success(n, 0))
                }
            })
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
async fn should_not_regenerate_schema_when_normalization_error_is_not_fixable() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let schema = shops_product_schema(id);
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc.expect_normalize().once().returning(|_, _, _| {
        Box::pin(async {
            Err(normalization_failure(
                NormalizationError::InvalidImageUrl {
                    raw: "not-a-url".to_string(),
                    source: url::Url::parse("://bad").unwrap_err(),
                },
                0,
            ))
        })
    });

    let cand_svc = MockScraperCandidateService::new();

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();
    assert!(matches!(err, ScraperError::NormalizationError(_)));
}

#[tokio::test]
async fn should_normalize_with_empty_images_when_image_policy_rejects_all_candidates() {
    let id = shop_id();
    let url = product_url();
    let html = r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="state">In Stock</span>
        <img src="/image-100x100.jpg">
        <img src="/image-120x120.jpg">
      </main>
    </body>
    </html>"#
        .to_string();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let schema = shops_product_schema(id);
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |raw, _, _| {
            let n = expected.clone();
            Box::pin(async move {
                assert!(raw.images.is_empty());
                Ok(normalization_success(n, 0))
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_keep_valid_image_fallback_after_malformed_candidate_without_schema_repair() {
    let id = shop_id();
    let url = product_url();
    let html = r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="state">In Stock</span>
        <img data-large_image="//" src="/image-800x600.jpg">
      </main>
    </body>
    </html>"#
        .to_string();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let mut product_schema = minimal_schema();
    product_schema.images = ExtractionRule {
        selector: CssSelector::from("img"),
        additional_selectors: vec![],
        extract: ExtractionKind::ImageUrl,
        cardinality: ExtractionCardinality::All,
    };
    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![product_schema],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |raw, _, _| {
            let n = expected.clone();
            Box::pin(async move {
                assert_eq!(raw.images, vec!["https://example.com/image-800x600.jpg"]);
                Ok(normalization_success(n, 0))
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_generate_single_schema_without_failed_schema_context() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

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
    let make_bad_schema = |title_selector: &str| ProductCssSelectorSchema {
        shops_product_id: Some(text_rule("non-existent-id")),
        title: text_rule(title_selector),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        seller_name: None,
        state: text_rule("non-existent-state"),
        images: attr_rule_all("img", "src"),
        auction_start: None,
        auction_end: None,
        default_currency: None,
        raw_attributes: Default::default(),
    };

    let bad_existing = make_bad_schema("non-existent-title-1");
    let bad_appended = make_bad_schema("non-existent-title-2");

    let mut schema_svc = MockProductSchemaService::new();
    let existing = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![bad_existing.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing.clone();
            Box::pin(async move { Ok(Some(s)) })
        });

    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let expected_bad_appended = bad_appended.clone();
            Box::pin(async move {
                Ok(generated_append_product(
                    expected_bad_appended,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();
    assert!(matches!(
        err,
        ScraperError::SchemaRegenerationExhausted {
            attempts: 1,
            last_error: ApplySchemaError::Title(ExtractionError::NoElementMatched { ref selector }),
            ..
        } if selector == "non-existent-title-2"
    ));
}

/// When the generated schema successfully applies on every attempt but
/// normalization still fails with a fixable error each time,
/// `fix_normalization_with_schema_retry` must exhaust its attempts and
/// return `FreshSchemaNormalizationFailed` rather than `SchemaRegenerationExhausted`.
#[tokio::test]
async fn should_return_normalization_fix_exhausted_when_schema_applies_but_norm_keeps_failing() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    // Schema is found in DB — no schema-gen LLM call on the obtain path.
    let existing_schema = minimal_schema();
    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![existing_schema.clone()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(generated_append_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    // Schema is never persisted because normalization never succeeds.
    schema_svc.expect_save_product_schemas().never();

    // Both normalize calls return TitleEmpty (fixable) so the single repair attempt is exhausted.
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(2) // 1 initial + 1 retry attempt
        .returning(|_, _, _| {
            Box::pin(async { Err(normalization_failure(NormalizationError::TitleEmpty, 0)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();
    assert!(
        matches!(
            err,
            ScraperError::FreshSchemaNormalizationFailed {
                attempts: 1,
                last_norm_error: NormalizationError::TitleEmpty,
                ..
            }
        ),
        "expected FreshSchemaNormalizationFailed, got {err:?}"
    );
}
