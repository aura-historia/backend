mod bookkeeping;
mod budget;
mod happy_path;
mod hash_skip;
mod normalization_fix;
mod seed_pages;

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

use crate::scraper::candidate_service::MockScraperCandidateService;
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, ShopsProductSchema};
use crate::scraper::css_selector::product_schema_service::MockProductSchemaService;
use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::MockProductNormalizationService;
use crate::scraper::scraper_service::ScraperService;
use crate::scraper::scraper_service::service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, MockHtmlFetcher, ScraperServiceImpl,
};
use crate::spider::classification::url_metadata::UrlState;
use common::language::domain::Language;
use common::localized::Localized;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use product::core::title::Title;
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
        shops_product_id: text_rule("#product-id"),
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
