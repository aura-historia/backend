use super::*;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::util::html::normalization_error_to_schema_hint;

#[test]
fn should_not_map_no_valid_images_to_schema_repair_hint() {
    assert!(
        normalization_error_to_schema_hint(&NormalizationError::NoValidImages { candidates: 2 })
            .is_none()
    );
}

#[test]
fn should_still_map_fixable_normalization_errors_to_schema_repair_hint() {
    assert!(matches!(
        normalization_error_to_schema_hint(&NormalizationError::TitleEmpty),
        Some(ApplySchemaError::Title(
            ExtractionError::NoElementMatched { .. }
        ))
    ));
}

#[tokio::test]
async fn should_regenerate_schema_when_normalization_error_is_fixable() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

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
    let existing_schema_for_append = existing_schema.clone();
    schema_svc
        .expect_append_single_schema()
        .once()
        .withf(move |_, prompt_source, failed_schema, last_error| {
            *prompt_source == SchemaPromptSource::YamlProjection
                && failed_schema == &Some(&existing_schema_for_append)
                && matches!(
                    last_error,
                    Some(ApplySchemaError::Title(ExtractionError::NoElementMatched {
                        selector
                    })) if selector == "title"
                )
        })
        .returning(move |_, _, _, _| {
            let s = minimal_schema();
            Box::pin(async move {
                Ok(generated_schemas(
                    vec![s],
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
                    Err(NormalizationError::TitleEmpty)
                } else {
                    Ok((n, 0u32))
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

    let result = service.scrape(&id, &url, None).await.unwrap().unwrap();
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
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

    let schema = shops_product_schema(id);
    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = schema.clone();
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc.expect_append_single_schema().never();
    schema_svc.expect_save_product_schemas().never();

    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc.expect_normalize().once().returning(|_, _, _| {
        Box::pin(async {
            Err(NormalizationError::InvalidImageUrl {
                raw: "not-a-url".to_string(),
                source: url::Url::parse("://bad").unwrap_err(),
            })
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

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(matches!(err, ScraperError::NormalizationError(_)));
}

#[tokio::test]
async fn should_not_regenerate_schema_when_image_policy_rejects_all_candidates() {
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
        Box::pin(async move { Ok(html) })
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
    schema_svc.expect_append_single_schema().never();
    schema_svc.expect_save_product_schemas().never();

    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc.expect_normalize().never();

    let cand_svc = MockScraperCandidateService::new();

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(matches!(
        err,
        ScraperError::NormalizationError(NormalizationError::NoValidImages { candidates: 2 })
    ));
}

#[tokio::test]
async fn should_pass_failed_schema_context_on_subsequent_retry_attempts() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

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
        shops_product_id: text_rule("non-existent-id"),
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

    let append_call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    schema_svc.expect_append_single_schema().times(2).returning(
        move |_, prompt_source, failed_schema, last_error| {
            let append_call_count = append_call_count.clone();
            let failed_schema = failed_schema.cloned();
            let last_error = last_error.cloned();
            let expected_bad_appended = bad_appended.clone();
            Box::pin(async move {
                let call = append_call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;

                match call {
                    1 => {
                        assert_eq!(prompt_source, SchemaPromptSource::YamlProjection);
                        assert!(failed_schema.is_none());
                        assert!(last_error.is_none());
                    }
                    2 => {
                        assert_eq!(prompt_source, SchemaPromptSource::CleanedHtmlFallback);
                        assert_eq!(failed_schema, Some(expected_bad_appended.clone()));
                        assert!(last_error.is_some());
                    }
                    _ => panic!("unexpected append attempt count: {call}"),
                }

                Ok(generated_schemas(
                    vec![expected_bad_appended],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        },
    );
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 2);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(matches!(
        err,
        ScraperError::SchemaRegenerationExhausted {
            attempts: 2,
            last_error: ApplySchemaError::ShopsProductId(ExtractionError::NoElementMatched { ref selector }),
            ..
        } if selector == "non-existent-id"
    ));
}

/// When the generated schema successfully applies on every attempt but
/// normalization still fails with a fixable error each time,
/// `fix_normalization_with_schema_retry` must exhaust its attempts and
/// return `NormalizationFixExhausted` rather than `SchemaRegenerationExhausted`.
#[tokio::test]
async fn should_return_normalization_fix_exhausted_when_schema_applies_but_norm_keeps_failing() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

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
    // `append_single_schema` is called once for YAML and once for cleaned HTML.
    schema_svc
        .expect_append_single_schema()
        .times(2)
        .returning(|_, _, _, _| {
            Box::pin(async {
                Ok(generated_schemas(
                    vec![minimal_schema()],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    // Schema is never persisted because normalization never succeeds.
    schema_svc.expect_save_product_schemas().never();

    // All 3 normalize calls return TitleEmpty (fixable) so the loop runs to exhaustion.
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .times(3) // 1 initial + 2 retry attempts
        .returning(|_, _, _| Box::pin(async { Err(NormalizationError::TitleEmpty) }));

    let mut cand_svc = MockScraperCandidateService::new();
    // 2 schema-generation LLM calls (one per attempt).
    expect_budget_increment(&mut cand_svc, 2);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(
        matches!(
            err,
            ScraperError::NormalizationFixExhausted {
                attempts: 2,
                last_norm_error: NormalizationError::TitleEmpty,
                ..
            }
        ),
        "expected NormalizationFixExhausted, got {err:?}"
    );
}
