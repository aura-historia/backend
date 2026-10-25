use super::*;
use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField, PlausibilityFinding};
use crate::scraper::raw_input::capture_source_listing_id;
use crate::scraper::scraper_service::ScrapedProduct;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::image_validation::{ImageValidation, ImageValidator};

/// A product page with elements a cached schema can mistakenly target.
fn page_with_implausible_elements() -> String {
    format!(
        r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <p id="description">Walnut veneer, circa 1830.</p>
        <span id="state">In Stock</span>
        <span id="tracking-title">Biedermeier Chair function(){{ track() }}</span>
        <div id="inline-css">Sold .badge {{ color: red; }}</div>
        <div id="long-state">{}</div>
        <span id="zero-price">£0.00</span>
        <img src="/images/chair-640x640.jpg">
      </main>
    </body>
    </html>"#,
        "Viewing by appointment ".repeat(10)
    )
}

fn text_rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

fn schema_with(edit: impl FnOnce(&mut ProductCssSelectorSchema)) -> ProductCssSelectorSchema {
    let mut schema = minimal_schema();
    edit(&mut schema);
    schema
}

fn fetcher_returning(html: String) -> MockHtmlFetcher {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let html = html.clone();
        Box::pin(async move { Ok(fetch_result(html)) })
    });
    fetcher
}

fn schema_service_with_cached(
    listing_source_id: ListingSourceId,
    schemas: Vec<ProductCssSelectorSchema>,
) -> MockProductListingSchemaService {
    let cached = ListingSourceProductSchema {
        listing_source_id,
        product_schemas: schemas,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(move |_| {
            let cached = cached.clone();
            Box::pin(async move { Ok(Some(cached)) })
        });
    schema_svc
}

fn expect_saved_fresh_schema(
    schema_svc: &mut MockProductListingSchemaService,
    listing_source_id: ListingSourceId,
    generated: ProductCssSelectorSchema,
) {
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let generated = generated.clone();
            Box::pin(async move {
                Ok(generated_single_product(
                    generated,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc
        .expect_save_product_schemas()
        .once()
        .returning(move |_, schemas| {
            Box::pin(async move {
                Ok(ListingSourceProductSchema {
                    listing_source_id,
                    product_schemas: schemas,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });
}

fn service(
    fetcher: MockHtmlFetcher,
    schema_svc: MockProductListingSchemaService,
    cand_svc: MockScraperCandidateService,
) -> ScraperServiceImpl {
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

fn captured_raw_value<'a>(scraped: &'a ScrapedProduct, field: &str) -> &'a serde_json::Value {
    &scraped.raw_input.raw_values().value()[field]
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
    let id = listing_source_id();
    let url = product_url();
    let rich_schema = schema_with(|schema| schema.description = Some(text_rule("main")));
    let mut schema_svc = schema_service_with_cached(id, vec![minimal_schema(), rich_schema]);
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    let fetcher = fetcher_returning(
        "<html><body><main><span id=\"product-id\">SKU-42</span><h1>Biedermeier Chair</h1><span id=\"state\">In Stock</span><img src=\"https://images.example/chair.jpg\"></main></body></html>".to_string(),
    );
    let image_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut service = service(fetcher, schema_svc, MockScraperCandidateService::new());
    service.image_validator = Box::new(CountingImageValidator(image_calls.clone()));

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a plausible cached candidate must be captured"));

    assert_eq!(image_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_ne!(
        captured_raw_value(&scraped, "description")["value"],
        serde_json::json!([]),
        "the richer candidate must be selected"
    );
}

/// The first cached schema extracts an implausible value from the page; the second,
/// plausible one is captured without fresh generation.
async fn assert_captures_second_cached_schema_after_rejecting(first: ProductCssSelectorSchema) {
    let id = listing_source_id();
    let url = product_url();
    let mut schema_svc = schema_service_with_cached(id, vec![first, minimal_schema()]);
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    let service = service(
        fetcher_returning(page_with_implausible_elements()),
        schema_svc,
        MockScraperCandidateService::new(),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the plausible cached candidate must be captured"));

    assert_eq!(
        captured_raw_value(&scraped, "title")["value"],
        "Biedermeier Chair"
    );
    assert_eq!(
        captured_raw_value(&scraped, "availability")["value"],
        "In Stock"
    );
    assert_eq!(
        captured_raw_value(&scraped, "description")["value"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn should_try_next_cached_schema_when_title_contains_script_source() {
    assert_captures_second_cached_schema_after_rejecting(schema_with(|schema| {
        schema.title = text_rule("#tracking-title");
    }))
    .await;
}

#[tokio::test]
async fn should_try_next_cached_schema_when_state_contains_inline_css() {
    assert_captures_second_cached_schema_after_rejecting(schema_with(|schema| {
        schema.state = text_rule("#inline-css");
    }))
    .await;
}

#[tokio::test]
async fn should_try_next_cached_schema_when_state_is_longer_than_a_label() {
    assert_captures_second_cached_schema_after_rejecting(schema_with(|schema| {
        schema.state = text_rule("#long-state");
    }))
    .await;
}

#[tokio::test]
async fn should_try_next_cached_schema_when_description_contains_inline_css() {
    assert_captures_second_cached_schema_after_rejecting(schema_with(|schema| {
        schema.description = Some(text_rule("#inline-css"));
    }))
    .await;
}

#[tokio::test]
async fn should_rank_cached_schema_with_zero_price_below_a_poorer_one_without_price() {
    let id = listing_source_id();
    let url = product_url();
    let zero_price = schema_with(|schema| {
        schema.price = Some(text_rule("#zero-price"));
        schema.description = Some(text_rule("#description"));
    });
    let mut schema_svc = schema_service_with_cached(id, vec![zero_price, minimal_schema()]);
    schema_svc.expect_generate_single_schema_for_page().never();
    let service = service(
        fetcher_returning(page_with_implausible_elements()),
        schema_svc,
        MockScraperCandidateService::new(),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a cached candidate must be captured"));

    assert_eq!(
        captured_raw_value(&scraped, "price"),
        &serde_json::json!({ "action": "CLEAR" })
    );
}

#[tokio::test]
async fn should_capture_url_hash_identity_when_the_extracted_identity_is_blank() {
    let id = listing_source_id();
    let url = product_url();
    let mut schema_svc = schema_service_with_cached(
        id,
        vec![schema_with(|schema| {
            schema.source_listing_id = Some(text_rule("#missing-product-id"));
        })],
    );
    schema_svc.expect_generate_single_schema_for_page().never();
    let service = service(
        fetcher_returning(sample_html()),
        schema_svc,
        MockScraperCandidateService::new(),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a blank identity must not reject the candidate"));

    let url_hash_identity = capture_source_listing_id("", url.as_str())
        .unwrap_or_else(|error| panic!("URL hash identity must derive: {error}"));
    assert_eq!(scraped.source_listing_id, url_hash_identity.to_string());
}

#[tokio::test]
async fn should_generate_fresh_schema_when_every_cached_extraction_is_implausible() {
    let id = listing_source_id();
    let url = product_url();
    let mut schema_svc = schema_service_with_cached(
        id,
        vec![
            schema_with(|schema| schema.state = text_rule("#inline-css")),
            schema_with(|schema| schema.title = text_rule("#tracking-title")),
        ],
    );
    expect_saved_fresh_schema(&mut schema_svc, id, minimal_schema());
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let service = service(
        fetcher_returning(page_with_implausible_elements()),
        schema_svc,
        cand_svc,
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the fresh schema must be captured"));

    assert_eq!(
        captured_raw_value(&scraped, "availability")["value"],
        "In Stock"
    );
}

/// A cached schema finds the title element but the page leaves it empty: no schema can
/// supply the title, so the page spends no LLM call, while a schema of another template
/// that matches nothing does not count.
#[tokio::test]
async fn should_report_missing_title_without_generation_when_the_title_element_is_empty() {
    let id = listing_source_id();
    let url = product_url();
    let other_template = schema_with(|schema| schema.title = text_rule("h1.product-name"));
    let mut schema_svc = schema_service_with_cached(id, vec![other_template, item_title_schema()]);
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_save_product_schemas().never();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_listing_source_llm_calls_with_limit()
        .never();
    let service = service(fetcher_returning(untitled_html()), schema_svc, cand_svc);

    let err = service.scrape(normal_request(&id, &url)).await.unwrap_err();

    let ScraperError::MissingRequiredValue {
        url: reported_url,
        findings,
    } = &err
    else {
        panic!("expected MissingRequiredValue, got {err:?}");
    };
    assert_eq!(reported_url, &url);
    assert_eq!(
        findings.as_slice(),
        [PlausibilityFinding {
            field: PlausibilityField::Title,
            check: PlausibilityCheck::Blank,
        }]
    );
    assert_eq!(
        err.to_string(),
        "Required value missing from 'https://example.com/products/123': `title` is blank"
    );
}

#[tokio::test]
async fn should_capture_without_images_when_image_policy_rejects_all_candidates() {
    let id = listing_source_id();
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
    let mut schema_svc = schema_service_with_cached(id, vec![minimal_schema()]);
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    let service = service(
        fetcher_returning(html),
        schema_svc,
        MockScraperCandidateService::new(),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("a page without valid images must be captured"));

    assert_eq!(
        captured_raw_value(&scraped, "images")["value"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn should_keep_valid_image_fallback_after_malformed_candidate() {
    let id = listing_source_id();
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
    let product_schema = schema_with(|schema| {
        schema.images = ExtractionRule {
            selector: CssSelector::from("img"),
            additional_selectors: vec![],
            extract: ExtractionKind::ImageUrl,
            cardinality: ExtractionCardinality::All,
        };
    });
    let mut schema_svc = schema_service_with_cached(id, vec![product_schema]);
    schema_svc.expect_generate_single_schema_for_page().never();
    schema_svc.expect_save_product_schemas().never();
    let service = service(
        fetcher_returning(html),
        schema_svc,
        MockScraperCandidateService::new(),
    );

    let scraped = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("the page must be captured"));

    assert_eq!(
        captured_raw_value(&scraped, "images")["value"],
        serde_json::json!(["https://example.com/image-800x600.jpg"])
    );
}

#[tokio::test]
async fn should_generate_single_schema_without_failed_schema_context() {
    let id = listing_source_id();
    let url = product_url();
    let make_bad_schema = |title_selector: &str| {
        schema_with(|schema| {
            schema.source_listing_id = Some(text_rule("non-existent-id"));
            schema.title = text_rule(title_selector);
            schema.state = text_rule("non-existent-state");
        })
    };
    let bad_generated = make_bad_schema("non-existent-title-2");
    let mut schema_svc =
        schema_service_with_cached(id, vec![make_bad_schema("non-existent-title-1")]);
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
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let service = service(fetcher_returning(sample_html()), schema_svc, cand_svc);

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();
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

/// A freshly generated schema that applies but extracts implausible values is
/// never persisted and reports the failing check per field.
#[tokio::test]
async fn should_fail_without_persisting_when_fresh_schema_extraction_is_implausible() {
    let id = listing_source_id();
    let url = product_url();
    let css_state = schema_with(|schema| schema.state = text_rule("#inline-css"));
    let mut schema_svc = schema_service_with_cached(id, vec![css_state.clone()]);
    schema_svc
        .expect_generate_single_schema_for_page()
        .once()
        .returning(move |_| {
            let generated = css_state.clone();
            Box::pin(async move {
                Ok(generated_single_product(
                    generated,
                    SchemaLlmEvaluationConfidence::High,
                ))
            })
        });
    schema_svc.expect_save_product_schemas().never();
    let mut cand_svc = MockScraperCandidateService::new();
    expect_budget_increment(&mut cand_svc, 1);
    let service = service(
        fetcher_returning(page_with_implausible_elements()),
        schema_svc,
        cand_svc,
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    let ScraperError::FreshSchemaImplausible { findings, .. } = &err else {
        panic!("expected FreshSchemaImplausible, got {err:?}");
    };
    assert_eq!(
        findings.as_slice(),
        [PlausibilityFinding {
            field: PlausibilityField::State,
            check: PlausibilityCheck::MarkupTokens,
        }]
    );
}
