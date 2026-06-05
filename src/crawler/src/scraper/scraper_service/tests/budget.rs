use super::*;
use crate::scraper::scraper_service::domain::errors::ScraperError;

#[tokio::test]
async fn should_return_llm_budget_exceeded_when_increment_is_rejected_on_schema_generation() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc.expect_create_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_shop_llm_calls_with_limit()
        .once()
        .returning(|_, _, _| Box::pin(async { Ok(false) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(matches!(
        err,
        ScraperError::LlmBudgetExceeded {
            shop_id,
            max_calls,
            ..
        } if shop_id == id && max_calls == DEFAULT_MAX_LLM_CALLS_PER_SHOP
    ));
}

/// When `normalize` reports 1 LLM call used (new state string hit the LLM
/// fallback), that call must be charged against the per-shop budget in
/// addition to the schema-generation call.
#[tokio::test]
async fn should_charge_budget_for_state_mapping_llm_call_when_normalization_uses_llm() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

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
            // Normalization used the LLM for state mapping (1 call).
            Box::pin(async move { Ok((n, 1u32)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    // Schema generation = 1 call, state mapping LLM = 1 call → 2 total.
    expect_budget_increment(&mut cand_svc, 2);
    expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let result = service.scrape(&id, &url, None).await.unwrap().unwrap();
    assert_eq!(result.product.state, ProductState::Available);
}

/// When the state mapping LLM call would push the shop over budget, the
/// scrape must return `LlmBudgetExceeded` even though the product was
/// successfully normalised.
#[tokio::test]
async fn should_return_llm_budget_exceeded_when_normalization_llm_call_exceeds_cap() {
    let id = shop_id();
    let url = product_url();

    let mut fetcher = MockHtmlFetcher::new();
    fetcher
        .expect_fetch()
        .once()
        .returning(|_| Box::pin(async { Ok(sample_html()) }));

    let schema = shops_product_schema(id);
    let schema_for_create = schema.product_schemas.first().cloned().unwrap();
    let mut schema_svc = MockProductSchemaService::new();
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
    // save_product_schemas IS called once during obtain_schemas (cache-miss
    // path), before normalization runs and the budget is rejected.
    let schema_for_save2 = schema.clone();
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, _| {
            let s = schema_for_save2.clone();
            Box::pin(async move { Ok(s) })
        });

    let expected = normalized_product(url.clone());
    let mut norm_svc = MockProductNormalizationService::new();
    norm_svc
        .expect_normalize()
        .once()
        .returning(move |_, _, _| {
            let n = expected.clone();
            // Normalization hit the LLM for state mapping.
            Box::pin(async move { Ok((n, 1u32)) })
        });

    let mut cand_svc = MockScraperCandidateService::new();
    // First call (schema gen) succeeds; second call (normalization LLM)
    // returns false → budget exhausted.
    let call_counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    cand_svc
        .expect_try_increment_shop_llm_calls_with_limit()
        .times(2)
        .returning(move |_, delta, _| {
            let counter = call_counter.clone();
            Box::pin(async move {
                let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(n == 0 && delta == 1)
            })
        });

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        3,
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();
    assert!(
        matches!(err, ScraperError::LlmBudgetExceeded { .. }),
        "expected LlmBudgetExceeded, got {err:?}"
    );
}
