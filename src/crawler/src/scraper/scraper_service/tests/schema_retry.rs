use super::*;
use crate::scraper::css_selector::product_schema::ShopsProductSchema;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;

fn invalid_schema() -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    schema.title.selector = CssSelector::from("missing-title");
    schema
}

fn existing_invalid_schema(shop_id: ShopId) -> ShopsProductSchema {
    ShopsProductSchema {
        shop_id,
        product_schemas: vec![invalid_schema()],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn fetcher_with_sample_html() -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));
    fetcher
}

fn normalizer_with_success(url: Url) -> MockProductNormalizationService {
    let expected = normalized_product(url);
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            Box::pin(async move { Ok((n, 0u32)) })
        });
    norm_svc
}

#[tokio::test]
async fn should_use_yaml_only_when_append_schema_applies() {
    let id = shop_id();
    let url = product_url();

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_append_single_schema()
        .once()
        .withf(|_, source, failed_schema, last_error| {
            *source == SchemaPromptSource::YamlProjection
                && failed_schema.is_none()
                && last_error.is_none()
        })
        .returning(|_, _, _, _| {
            Box::pin(async {
                Ok(generated_schemas(
                    vec![minimal_schema()],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            Box::pin(async move {
                Ok(ShopsProductSchema {
                    shop_id: id,
                    product_schemas: schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(normalizer_with_success(url.clone())),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_use_cleaned_html_fallback_when_yaml_append_does_not_apply() {
    let id = shop_id();
    let url = product_url();

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });

    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    schema_svc.expect_append_single_schema().times(2).returning(
        move |_, source, failed_schema, last_error| {
            let call_count = call_count.clone();
            let failed_schema = failed_schema.cloned();
            let last_error = last_error.cloned();
            Box::pin(async move {
                let call = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match call {
                    0 => {
                        assert_eq!(source, SchemaPromptSource::YamlProjection);
                        assert!(failed_schema.is_none());
                        assert!(last_error.is_none());
                        Ok(generated_schemas(
                            vec![invalid_schema()],
                            SchemaLlmEvaluationConfidence::High,
                        ))
                    }
                    1 => {
                        assert_eq!(source, SchemaPromptSource::CleanedHtmlFallback);
                        assert_eq!(failed_schema, Some(invalid_schema()));
                        assert!(last_error.is_some());
                        Ok(generated_schemas(
                            vec![minimal_schema()],
                            SchemaLlmEvaluationConfidence::High,
                        ))
                    }
                    _ => panic!("unexpected append call {call}"),
                }
            })
        },
    );
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            Box::pin(async move {
                Ok(ShopsProductSchema {
                    shop_id: id,
                    product_schemas: schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 2);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(normalizer_with_success(url.clone())),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_exhaust_append_repair_after_yaml_and_cleaned_html_fail() {
    let id = shop_id();
    let url = product_url();

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_append_single_schema()
        .times(2)
        .returning(|_, _, _, _| {
            Box::pin(async {
                Ok(generated_schemas(
                    vec![invalid_schema()],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 2);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
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
            last_error: crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched { .. }
            ),
            ..
        }
    ));
}

#[tokio::test]
async fn should_not_call_cleaned_html_fallback_when_budget_is_exhausted_before_fallback() {
    let id = shop_id();
    let url = product_url();

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    schema_svc
        .expect_append_single_schema()
        .once()
        .withf(|_, source, _, _| *source == SchemaPromptSource::YamlProjection)
        .returning(|_, _, _, _| {
            Box::pin(async {
                Ok(generated_schemas(
                    vec![invalid_schema()],
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    cand_svc
        .expect_try_increment_shop_llm_calls_with_limit()
        .times(2)
        .returning(move |_, _, _| {
            let call_count = call_count.clone();
            Box::pin(async move {
                let call = call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(call == 0)
            })
        });

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(matches!(err, ScraperError::LlmBudgetExceeded { .. }));
}
