use super::*;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::image_validation::{ImageValidation, ImageValidator};

#[test]
fn should_reject_no_valid_images_as_candidate_data() {
    assert_eq!(
        NormalizationError::NoValidImages { candidates: 2 }.failure_scope(),
        NormalizationFailureScope::CandidateData
    );
}

#[test]
fn should_classify_title_errors_as_cached_schema_fallback_failures() {
    assert_eq!(
        NormalizationError::TitleEmpty.failure_scope(),
        NormalizationFailureScope::CandidateData
    );

    assert_eq!(
        NormalizationError::StateMappingError(
            crate::scraper::normalization::state_mapping_service::StateMappingServiceError::DatabaseError(
                sqlx::Error::RowNotFound,
            ),
        )
        .failure_scope(),
        NormalizationFailureScope::External
    );
}

struct CountingImageValidator(std::sync::Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl ImageValidator for CountingImageValidator {
    async fn validate(&self, _url: &url::Url) -> ImageValidation {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ImageValidation::Valid
    }
}

#[tokio::test]
async fn should_validate_images_before_ranking_all_cached_candidates() {
    let id = shop_id();
    let url = product_url();
    let mut rich_schema = minimal_schema();
    rich_schema.description = Some(ExtractionRule {
        selector: CssSelector::from("main"),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    });
    let schema = ShopsProductSchema {
        shop_id: id,
        product_schemas: vec![minimal_schema(), rich_schema],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| {
            Box::pin(async {
                Ok(fetch_result(
                    "<html><body><main><span id=\"product-id\">SKU-42</span><h1>Biedermeier Chair</h1><span id=\"state\">In Stock</span><img src=\"https://images.example/chair.jpg\"></main></body></html>".to_string(),
                ))
            })
        });
    let mut schema_svc = MockProductListingSchemaService::new();
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
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |raw, _, _| {
            assert!(raw.description.iter().any(|value| !value.is_empty()));
            let product = expected.clone();
            Box::pin(async move { Ok(normalization_success(product, 0)) })
        });

    let mut candidate_svc = MockScraperCandidateService::new();
    expect_successful_bookkeeping(&mut candidate_svc, id, url.clone(), UrlState::Available);
    let image_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        std::sync::Arc::new(candidate_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );
    service.image_validator = Box::new(CountingImageValidator(image_calls.clone()));

    assert!(
        service
            .scrape(&id, &url, None, None)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(image_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

async fn assert_tries_next_cached_schema_after(error: NormalizationError) {
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

    let expected = normalized_product(url.clone());
    let first_error = Arc::new(std::sync::Mutex::new(Some(error)));
    let expected_scope = first_error
        .lock()
        .expect("first error lock should not be poisoned")
        .as_ref()
        .expect("first error should be present")
        .failure_scope();
    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(1..=2)
        .returning(move |_, _, _| {
            let n = expected.clone();
            let first_error = first_error.clone();
            Box::pin(async move {
                let error = first_error
                    .lock()
                    .expect("first error lock should not be poisoned")
                    .take();
                match error {
                    Some(error) => Err(normalization_failure(error, 0)),
                    None => Ok(normalization_success(n, 0)),
                }
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    if expected_scope == NormalizationFailureScope::CandidateData {
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);
    }

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None, None).await;
    match expected_scope {
        NormalizationFailureScope::CandidateData => {
            let product = result.unwrap().unwrap();
            assert_eq!(
                product.product.shop_listing_id,
                ShopListingId::from("SKU-42")
            );
        }
        NormalizationFailureScope::External => {
            assert!(matches!(result, Err(ScraperError::NormalizationError(_))));
        }
    }
}

#[tokio::test]
async fn should_try_next_cached_schema_after_title_failure() {
    assert_tries_next_cached_schema_after(NormalizationError::TitleEmpty).await;
}

#[tokio::test]
async fn should_try_next_cached_schema_after_description_language_failure() {
    assert_tries_next_cached_schema_after(NormalizationError::DescriptionUnknownLanguage {
        text: "garbage".to_string(),
    })
    .await;
}

#[tokio::test]
async fn should_try_next_cached_schema_after_auction_start_failure() {
    assert_tries_next_cached_schema_after(NormalizationError::AuctionStartParseError {
        raw: "garbage".to_string(),
    })
    .await;
}

#[tokio::test]
async fn should_try_next_cached_schema_after_price_failure() {
    assert_tries_next_cached_schema_after(NormalizationError::PriceParseError {
        raw: "garbage".to_string(),
    })
    .await;
}

#[tokio::test]
async fn should_try_all_cached_schemas_before_fresh_generation() {
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
    let mut schema_svc = MockProductListingSchemaService::new();
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
                Ok(generated_single_product(
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
    let mut norm_svc = MockProductListingNormalizationService::new();
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
        result.product.shop_listing_id,
        ShopListingId::from("SKU-42")
    );
}

#[tokio::test]
async fn should_generate_fresh_schema_when_cached_data_fails() {
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
    let mut schema_svc = MockProductListingSchemaService::new();
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
                Ok(generated_single_product(
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
    let mut norm_svc = MockProductListingNormalizationService::new();
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
        result.product.shop_listing_id,
        ShopListingId::from("SKU-42")
    );
}

#[tokio::test]
async fn should_abort_without_fresh_schema_when_cached_candidate_has_external_failure() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));

    let schema = shops_product_schema(id);
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

    let mut norm_svc = MockProductListingNormalizationService::new();
    norm_svc.expect_normalize().once().returning(|_, _, _| {
        Box::pin(async {
            Err(normalization_failure(
                NormalizationError::StateMappingError(
                    crate::scraper::normalization::state_mapping_service::StateMappingServiceError::DatabaseError(
                        sqlx::Error::RowNotFound,
                    ),
                ),
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
    let mut schema_svc = MockProductListingSchemaService::new();
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
    let mut norm_svc = MockProductListingNormalizationService::new();
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
async fn should_keep_valid_image_fallback_after_malformed_candidate() {
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
    let mut schema_svc = MockProductListingSchemaService::new();
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
    let mut norm_svc = MockProductListingNormalizationService::new();
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
        shop_listing_id: Some(text_rule("non-existent-id")),
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
    let bad_generated = make_bad_schema("non-existent-title-2");

    let mut schema_svc = MockProductListingSchemaService::new();
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
            let expected_bad_generated = bad_generated.clone();
            Box::pin(async move {
                Ok(generated_single_product(
                    expected_bad_generated,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductListingNormalizationService::new();
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
            ref last_error,
            ..
        } if matches!(
            last_error.as_ref(),
            ApplySchemaError::Title(ExtractionError::NoElementMatched { selector })
                if selector == "non-existent-title-2"
        )
    ));
}

/// When the generated schema successfully applies on every attempt but
/// normalization still fails with candidate-data errors each time, fresh
/// generation must return `FreshSchemaNormalizationFailed`.
#[tokio::test]
async fn should_fail_when_fresh_schema_normalization_keeps_failing() {
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
    let mut schema_svc = MockProductListingSchemaService::new();
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
                Ok(generated_single_product(
                    minimal_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    // Schema is never persisted because normalization never succeeds.
    schema_svc.expect_save_product_schemas().never();

    // Both normalize calls return candidate-data failures.
    let mut norm_svc = MockProductListingNormalizationService::new();
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
                ref last_norm_error,
                ..
            } if matches!(last_norm_error.as_ref(), NormalizationError::TitleEmpty)
        ),
        "expected FreshSchemaNormalizationFailed, got {err:?}"
    );
}
