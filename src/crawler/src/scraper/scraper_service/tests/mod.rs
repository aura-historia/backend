use shop_core::shop_id::ShopId;
mod bookkeeping;
mod budget;
mod happy_path;
mod hash_skip;
mod normalization_fix;
mod redirect_guard;
mod removed_page;
mod schema_fallback;
mod schema_retry;
mod seed_pages;

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

use crate::scraper::candidate_service::MockScraperCandidateService;
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, ShopsProductSchema};
use crate::scraper::css_selector::product_schema_service::{
    GeneratedAppendSchema, GeneratedProductSchemas, MockProductSchemaService, SchemaLlmEvaluation,
    SchemaLlmEvaluationConfidence, SchemaLlmEvaluationDecision,
};
use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::{
    MockProductNormalizationService, NormalizationFailure, NormalizationSuccess,
};
use crate::scraper::scraper_service::ScraperService;
use crate::scraper::scraper_service::service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, FetchedHtml, MockHtmlFetcher, ScraperServiceImpl,
};
use crate::spider::classification::url_metadata::UrlState;
use localization::Language;
use localization::Localized;
use product_core::product_state::ProductState;
use product_core::shops_product_id::ShopsProductId;
use product_core::title::Title;
use std::sync::Arc;
use time::OffsetDateTime;
use url::Url;

pub(super) fn shop_id() -> ShopId {
    ShopId::new()
}

pub(super) fn product_url() -> Url {
    Url::parse("https://example.com/products/123").unwrap()
}

pub(super) fn sample_html() -> String {
    r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="state">In Stock</span>
        <img src="/images/chair-640x640.jpg">
      </main>
    </body>
    </html>"#
        .to_string()
}

pub(super) fn fetch_result(html: String) -> FetchedHtml {
    FetchedHtml::new(html, product_url())
}

pub(super) fn fetch_result_for(html: String, final_url: Url) -> FetchedHtml {
    FetchedHtml::new(html, final_url)
}

pub(super) fn minimal_schema() -> ProductCssSelectorSchema {
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
        shops_product_id: Some(text_rule("#product-id")),
        title: text_rule("h1"),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        seller_name: None,
        state: text_rule("#state"),
        images: attr_rule_all("img", "src"),
        auction_start: None,
        auction_end: None,
        default_currency: None,
        raw_attributes: Default::default(),
    }
}

pub(super) fn shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
    let schema = minimal_schema();
    ShopsProductSchema {
        shop_id,
        product_schemas: vec![schema],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

pub(super) fn generated_schemas(
    schemas: Vec<ProductCssSelectorSchema>,
    confidence: SchemaLlmEvaluationConfidence,
) -> GeneratedProductSchemas {
    GeneratedProductSchemas {
        schemas,
        evaluation: SchemaLlmEvaluation {
            decision: if confidence == SchemaLlmEvaluationConfidence::High {
                SchemaLlmEvaluationDecision::Approve
            } else {
                SchemaLlmEvaluationDecision::NeedsHumanReview
            },
            confidence,
            approved_by_llm: false,
            summary: "Selectors are product-specific.".to_string(),
            risks: Vec::new(),
        },
    }
}

pub(super) fn schema_evaluation(confidence: SchemaLlmEvaluationConfidence) -> SchemaLlmEvaluation {
    SchemaLlmEvaluation {
        decision: if confidence == SchemaLlmEvaluationConfidence::High {
            SchemaLlmEvaluationDecision::Approve
        } else {
            SchemaLlmEvaluationDecision::NeedsHumanReview
        },
        confidence,
        approved_by_llm: false,
        summary: "Selectors are product-specific.".to_string(),
        risks: Vec::new(),
    }
}

pub(super) fn generated_append_product(
    schema: ProductCssSelectorSchema,
    confidence: SchemaLlmEvaluationConfidence,
) -> GeneratedAppendSchema {
    GeneratedAppendSchema::Product {
        schema: Box::new(schema),
        evaluation: schema_evaluation(confidence),
    }
}

pub(super) fn normalized_product(url: Url) -> NormalizedProduct {
    let title: Title = "Biedermeier Chair".into();
    NormalizedProduct {
        shops_product_id: ShopsProductId::from("SKU-42"),
        title: Localized::new(Language::De, title),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        seller_name: None,
        state: ProductState::Available,
        url,
        images: vec![],
        auction_start: None,
        auction_end: None,
        raw_attributes: Default::default(),
    }
}

pub(super) fn normalization_success(
    product: NormalizedProduct,
    llm_calls_used: u32,
) -> NormalizationSuccess {
    NormalizationSuccess {
        product,
        llm_calls_used,
    }
}

pub(super) fn normalization_failure(
    error: NormalizationError,
    llm_calls_used: u32,
) -> NormalizationFailure {
    NormalizationFailure {
        error,
        llm_calls_used,
    }
}

pub(super) fn expect_successful_bookkeeping(
    cand_svc: &mut MockScraperCandidateService,
    shop_id: ShopId,
    url: Url,
    state: UrlState,
) {
    let url_for_set_state = url.clone();
    cand_svc
        .expect_set_state()
        .once()
        .withf(move |received_shop_id, received_url, received_state| {
            *received_shop_id == shop_id
                && received_url == &url_for_set_state
                && *received_state == state
        })
        .returning(|_, _, _| Box::pin(async { Ok(()) }));
}

pub(super) fn expect_budget_increment(cand_svc: &mut MockScraperCandidateService, times: usize) {
    cand_svc
        .expect_try_increment_shop_llm_calls_with_limit()
        .times(times)
        .returning(|_, _, _| Box::pin(async { Ok(true) }));
}
