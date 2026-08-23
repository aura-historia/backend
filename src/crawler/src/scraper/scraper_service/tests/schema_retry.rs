use super::*;
use crate::scraper::css_selector::product_schema::ShopsProductSchema;
use crate::scraper::css_selector::product_schema_service::GeneratedAppendSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema_repository::MockRemovedPageSchemaRepository;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::spider::classification::url_metadata::UrlClass;
use shop_core::shop_id::ShopId;

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
        .returning(|_| Box::pin(async { Ok(fetch_result(sample_html())) }));
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
            Box::pin(async move { Ok(normalization_success(n, 0)) })
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
        .returning(|_| {
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

    let result = service.scrape(&id, &url, None, None).await.unwrap();
    assert!(result.is_some());
}

#[tokio::test]
async fn should_exhaust_append_repair_when_yaml_append_does_not_apply() {
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
        .returning(|_| {
            Box::pin(async {
                Ok(generated_append_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(MockProductNormalizationService::new()),
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
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched { .. }
            )
        )
    ));
}

#[tokio::test]
async fn should_exhaust_append_repair_after_yaml_fails() {
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
        .returning(|_| {
            Box::pin(async {
                Ok(generated_append_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
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
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched { .. }
            )
        )
    ));
}

#[tokio::test]
async fn should_not_consume_second_budget_call_when_yaml_append_does_not_apply() {
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
        .returning(|_| {
            Box::pin(async {
                Ok(generated_append_product(
                    invalid_schema(),
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let norm_svc = MockProductNormalizationService::new();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_shop_llm_calls_with_limit()
        .once()
        .returning(move |_, _, _| Box::pin(async move { Ok(true) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher_with_sample_html()),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();
    assert!(matches!(
        err,
        ScraperError::SchemaRegenerationExhausted { attempts: 1, .. }
    ));
}

#[tokio::test]
async fn should_mark_removed_when_append_classifies_removed() {
    let id = shop_id();
    let url = product_url();
    let removed_html = r#"<main><h1 id="removed-message">Product no longer available</h1></main>"#;
    let removed_schema = RemovedPageSchema {
        selector: CssSelector::from("#removed-message"),
        text: Some("Product no longer available".to_string()),
        regex: None,
    };

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = removed_html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

    let mut schema_svc = MockProductSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let s = existing_invalid_schema(id);
            Box::pin(async move { Ok(Some(s)) })
        });
    let removed_schema_for_append = removed_schema.clone();
    schema_svc
        .expect_append_single_schema()
        .once()
        .returning(move |_| {
            let schema = removed_schema_for_append.clone();
            Box::pin(async move {
                Ok(GeneratedAppendSchema::Removed {
                    schema,
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut removed_repo = MockRemovedPageSchemaRepository::new();
    removed_repo
        .expect_find_removed_page_schema()
        .times(2)
        .returning(|_| Box::pin(async { Ok(None) }));
    removed_repo
        .expect_insert_removed_page_schema()
        .once()
        .withf(move |received_shop_id, row| {
            *received_shop_id == id && row.removed_page_schemas == vec![removed_schema.clone()]
        })
        .returning(|_, row| {
            let row = row.clone();
            Box::pin(async move { Ok(row) })
        });
    removed_repo.expect_update_removed_page_schema().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let url_for_state = url.clone();
    cand_svc
        .expect_set_state()
        .once()
        .withf(move |received_shop_id, received_url, received_state| {
            *received_shop_id == id
                && received_url == &url_for_state
                && *received_state == UrlState::Removed
        })
        .returning(|_, _, _| Box::pin(async { Ok(()) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(MockProductNormalizationService::new()),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    )
    .with_removed_page_schema_repository(Box::new(removed_repo));

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();

    assert!(matches!(err, ScraperError::ProductRemoved { .. }));
}

#[tokio::test]
async fn should_mark_other_when_append_classifies_not_product() {
    let id = shop_id();
    let url = product_url();
    let category_html = r#"<main class="category"><h1>Latest antiques</h1></main>"#;

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = category_html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

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
        .returning(move |_| {
            Box::pin(async move {
                Ok(GeneratedAppendSchema::NotProduct {
                    reason: "category page".to_string(),
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let url_for_class = url.clone();
    cand_svc
        .expect_set_class()
        .once()
        .withf(move |received_shop_id, received_url, received_class| {
            *received_shop_id == id
                && received_url == &url_for_class
                && *received_class == UrlClass::Other
        })
        .returning(|_, _, _| Box::pin(async { Ok(()) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(MockProductNormalizationService::new()),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();

    assert!(matches!(err, ScraperError::NotProductPage { .. }));
}

#[tokio::test]
async fn should_not_change_state_or_class_when_append_classification_does_not_match_html() {
    let id = shop_id();
    let url = product_url();
    let html = r#"<main><h1>Still a weird page</h1></main>"#;

    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.to_string();
        Box::pin(async move { Ok(fetch_result(html)) })
    });

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
        .returning(|_| {
            Box::pin(async {
                Ok(GeneratedAppendSchema::Removed {
                    schema: RemovedPageSchema {
                        selector: CssSelector::from("#missing"),
                        text: Some("Product no longer available".to_string()),
                        regex: None,
                    },
                    evaluation: schema_evaluation(SchemaLlmEvaluationConfidence::High),
                })
            })
        });
    schema_svc.expect_save_product_schemas().never();

    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    cand_svc.expect_set_state().never();
    cand_svc.expect_set_class().never();

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(MockProductNormalizationService::new()),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None, None).await.unwrap_err();

    assert!(matches!(
        err,
        ScraperError::SchemaRegenerationExhausted { attempts: 1, .. }
    ));
}
