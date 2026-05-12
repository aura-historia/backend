use crate::scraper::css_selector::currency_dto::CurrencyDto;
use crate::scraper::css_selector::rule::{ExtractionError, ExtractionRule};
use common::shop_id::ShopId;
use llm::chat::StructuredOutputFormat;
use schemars::JsonSchema;
use scraper::Html;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopsProductSchema {
    pub shop_id: ShopId,
    pub product_schemas: Vec<ProductCssSelectorSchema>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ShopsProductSchema {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ShopsProductSchema {
                shop_id: config.fake_with_rng(rng),
                product_schemas: vec![],
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Schema of rules for extracting product information from a shop's website using CSS selectors.
    Each field represents a specific piece of information about the product, and the value is an ExtractionRule that defines how to extract that information from the HTML of the shop's website.
    The rules are intended to extract raw data from the HTML, not normalized data."
)]
pub struct ProductCssSelectorSchema {
    #[schemars(description = "ID of the product on the shop's website")]
    pub shops_product_id: ExtractionRule,

    #[schemars(description = "Title of the product")]
    pub title: ExtractionRule,

    #[schemars(
        description = "Main product description/body content. May be fragmented across multiple nodes. Prefer the central product-description area for this item. Avoid shipping info, legal disclaimers, navigation text, marketing banners, and recommendation or related-products sections."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<ExtractionRule>,

    #[schemars(
        description = "Visible product price text. Prefer the actual price element for this product and extract human-visible text, not attributes. Avoid wrapper containers, struck-through comparison prices, totals for other products, shipping costs, and unrelated price widgets."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<ExtractionRule>,

    #[schemars(
        description = "Lower bound of an explicitly shown estimate price range for this product. Use only when the page clearly presents an estimate minimum/bound. Avoid deriving it from a single sale price, bid price, or unrelated range/filter widget."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<ExtractionRule>,

    #[schemars(
        description = "Upper bound of an explicitly shown estimate price range for this product. Use only when the page clearly presents an estimate maximum/bound. Avoid deriving it from a single sale price, bid price, or unrelated range/filter widget."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<ExtractionRule>,

    #[schemars(
        description = "Availability state of the product. E.g. 'in stock', 'out of stock', 'preorder', 'add to cart', etc. Prioritize state sources in this order: (1) clear explicit state text such as 'available', 'sold', or 'out of stock'; (2) visible text from a product-specific add-to-cart or buy button; (3) visible text from other product-specific buttons that clearly indicate availability such as preorder, reserve, or sold-out actions. Prefer dedicated availability labels or visible button text over generic class names or whole script blobs. IMPORTANT: Never use price elements, image galleries, or generic layout wrappers as the state selector."
    )]
    pub state: ExtractionRule,

    #[schemars(
        description = "Product media URLs. May be fragmented across multiple gallery nodes. Prefer canonical product image/media URLs from src, srcset, href-like media links, or gallery-specific attributes. Avoid logos, icons, placeholders, sprites, and unrelated thumbnails from navigation or recommendations."
    )]
    pub images: ExtractionRule,

    #[schemars(
        description = "Auction start date/time for this product. Prefer machine-readable datetime-bearing nodes such as time[datetime], structured data, or clearly labeled auction metadata. Avoid generic date text unless it clearly refers to the auction start timestamp for this product."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction_start: Option<ExtractionRule>,

    #[schemars(
        description = "Auction end date/time for this product. Prefer machine-readable datetime-bearing nodes such as time[datetime], structured data, or clearly labeled auction metadata. Avoid generic date text unless it clearly refers to the auction end timestamp for this product."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction_end: Option<ExtractionRule>,

    #[schemars(
        description = "The default currency for this shop's prices, as an ISO 4217 code (e.g. \
        \"EUR\", \"GBP\", \"USD\", \"AUD\", \"CAD\", \"NZD\"). \
        This is full-page fallback context, not a selector rule. \
        Set this when the price elements on the page do not include a currency symbol or code \
        themselves — for example when the currency appears in a sibling element \
        (e.g. <span class=\"currency\">EUR</span>), a page-level label \
        (\"Auction currency: EUR\"), a <meta> tag, or structured data (JSON-LD / microdata). \
        Leave null only if the currency is always embedded in every price string."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_currency: Option<CurrencyDto>,
}

/// Errors that can occur when applying a [`ProductCssSelectorSchema`] to an HTML document.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ApplySchemaError {
    #[error("failed to extract `shops_product_id`: {0}")]
    ShopsProductId(#[source] ExtractionError),

    #[error("failed to extract `title`: {0}")]
    Title(#[source] ExtractionError),

    #[error("failed to extract `description`: {0}")]
    Description(#[source] ExtractionError),

    #[error("failed to extract `price`: {0}")]
    Price(#[source] ExtractionError),

    #[error("failed to extract `price_estimate_min`: {0}")]
    PriceEstimateMin(#[source] ExtractionError),

    #[error("failed to extract `price_estimate_max`: {0}")]
    PriceEstimateMax(#[source] ExtractionError),

    #[error("failed to extract `state`: {0}")]
    State(#[source] ExtractionError),

    #[error("failed to extract `images`: {0}")]
    Images(#[source] ExtractionError),

    #[error("failed to extract `auction_start`: {0}")]
    AuctionStart(#[source] ExtractionError),

    #[error("failed to extract `auction_end`: {0}")]
    AuctionEnd(#[source] ExtractionError),
}

impl ProductCssSelectorSchema {
    /// Apply all extraction rules in this schema to the given parsed HTML document,
    /// returning a [`RawExtractedProduct`] with the raw (non-normalised) values.
    ///
    /// Rules for optional fields are skipped (returning `None`) when the field itself
    /// is `None`. When a field is present but its rule fails (e.g. no element matched),
    /// the corresponding [`ApplySchemaError`] variant is returned immediately.
    ///
    /// For single-valued fields (`shops_product_id`, `title`, `state`) the first
    /// element of the extraction result is used. For multi-valued fields
    /// (`description`, `images`) all results are kept as a `Vec<String>`.
    pub fn apply(&self, html: &Html) -> Result<RawExtractedProduct, ApplySchemaError> {
        let shops_product_id = self
            .shops_product_id
            .apply(html)
            .map_err(ApplySchemaError::ShopsProductId)?
            .into_iter()
            .next()
            .unwrap_or_default();

        let title = self
            .title
            .apply(html)
            .map_err(ApplySchemaError::Title)?
            .into_iter()
            .next()
            .unwrap_or_default();

        let description = match &self.description {
            None => vec![],
            Some(rule) => match rule.apply(html) {
                Ok(vals) => vals,
                Err(e) => return Err(ApplySchemaError::Description(e)),
            },
        };

        let price = match &self.price {
            None => None,
            Some(rule) => match rule.apply(html) {
                Ok(vals) => Some(vals.into_iter().next().unwrap_or_default()),
                Err(e) => return Err(ApplySchemaError::Price(e)),
            },
        };

        let price_estimate_min = match &self.price_estimate_min {
            None => None,
            Some(rule) => match rule.apply(html) {
                Ok(vals) => Some(vals.into_iter().next().unwrap_or_default()),
                Err(e) => return Err(ApplySchemaError::PriceEstimateMin(e)),
            },
        };

        let price_estimate_max = match &self.price_estimate_max {
            None => None,
            Some(rule) => match rule.apply(html) {
                Ok(vals) => Some(vals.into_iter().next().unwrap_or_default()),
                Err(e) => return Err(ApplySchemaError::PriceEstimateMax(e)),
            },
        };

        let state = self
            .state
            .apply(html)
            .map_err(ApplySchemaError::State)?
            .into_iter()
            .next()
            .unwrap_or_default();

        let images = self.images.apply(html).map_err(ApplySchemaError::Images)?;

        let auction_start = match &self.auction_start {
            None => None,
            Some(rule) => match rule.apply(html) {
                Ok(vals) => Some(vals.into_iter().next().unwrap_or_default()),
                Err(e) => return Err(ApplySchemaError::AuctionStart(e)),
            },
        };

        let auction_end = match &self.auction_end {
            None => None,
            Some(rule) => match rule.apply(html) {
                Ok(vals) => Some(vals.into_iter().next().unwrap_or_default()),
                Err(e) => return Err(ApplySchemaError::AuctionEnd(e)),
            },
        };

        Ok(RawExtractedProduct {
            shops_product_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            state,
            images,
            auction_start,
            auction_end,
        })
    }

    pub fn structured_output_format() -> StructuredOutputFormat {
        let schema = schemars::schema_for!(ProductCssSelectorSchema);
        let schema_json = serde_json::to_value(&schema).expect(
            "shouldn't fail serializing schema-rs for ProductCssSelectorSchema to json-value",
        );
        StructuredOutputFormat {
            name: "ProductCssSelectorSchema".to_string(),
            description: None,
            schema: Some(schema_json),
            strict: Some(true),
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawExtractedProduct {
    pub shops_product_id: String,
    pub title: String,
    pub description: Vec<String>,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub state: String,
    pub images: Vec<String>,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use scraper::Html;

    use crate::scraper::css_selector::product_schema::{
        ApplySchemaError, ProductCssSelectorSchema,
    };
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule, HtmlAttributeName,
    };

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn text_rule_all(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::All,
        }
    }

    fn attr_rule(selector: &str, attr: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute {
                name: HtmlAttributeName::from(attr),
            },
            cardinality: ExtractionCardinality::First,
        }
    }

    fn attr_rule_all(selector: &str, attr: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute {
                name: HtmlAttributeName::from(attr),
            },
            cardinality: ExtractionCardinality::All,
        }
    }

    /// Minimal valid schema covering only the mandatory fields.
    fn minimal_schema(html: &str) -> (Html, ProductCssSelectorSchema) {
        let parsed = Html::parse_document(html);
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        (parsed, schema)
    }

    /// A small but realistic product-page HTML fragment used across many tests.
    fn product_html() -> &'static str {
        r#"<!DOCTYPE html>
<html>
<body>
  <span id="product-id">SKU-42</span>
  <h1>Biedermeier Chair</h1>
  <p class="desc">A beautiful antique chair</p>
  <p class="desc">Circa 1830, walnut wood</p>
  <span class="price">€ 1.200</span>
  <span id="state">In Stock</span>
  <img src="/images/chair-front.jpg" alt="front">
  <img src="/images/chair-side.jpg" alt="side">
  <time id="auction-start" datetime="2025-06-01T10:00:00Z">June 1</time>
  <time id="auction-end"   datetime="2025-06-07T18:00:00Z">June 7</time>
</body>
</html>"#
    }

    fn full_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: Some(text_rule_all("p.desc")),
            price: Some(text_rule("span.price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: Some(attr_rule("time#auction-start", "datetime")),
            auction_end: Some(attr_rule("time#auction-end", "datetime")),
            default_currency: None,
        }
    }

    // -------------------------------------------------------------------------
    // Schema / structured output (pre-existing tests, kept here)
    // -------------------------------------------------------------------------

    #[test]
    fn should_create_schema() {
        let _schema = schemars::schema_for!(ProductCssSelectorSchema);
    }

    #[test]
    fn should_create_structured_output_format() {
        let _structured_output_format = ProductCssSelectorSchema::structured_output_format();
    }

    // -------------------------------------------------------------------------
    // Happy-path: mandatory fields
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_shops_product_id_when_element_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(result.shops_product_id, "SKU-42");
    }

    #[test]
    fn should_extract_title_when_h1_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(result.title, "Biedermeier Chair");
    }

    #[test]
    fn should_extract_state_when_element_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(result.state, "In Stock");
    }

    #[test]
    fn should_extract_all_images_when_multiple_img_elements_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(
            result.images,
            vec!["/images/chair-front.jpg", "/images/chair-side.jpg"]
        );
    }

    // -------------------------------------------------------------------------
    // Happy-path: optional fields present
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_description_fragments_when_rule_present_and_multiple_elements_match() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(
            result.description,
            vec!["A beautiful antique chair", "Circa 1830, walnut wood"]
        );
    }

    #[test]
    fn should_extract_price_when_rule_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(result.price, Some("€ 1.200".to_string()));
    }

    #[test]
    fn should_extract_price_estimate_min_when_rule_present() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="a.jpg">
                <span id="est-min">800</span>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: Some(text_rule("#est-min")),
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.price_estimate_min, Some("800".to_string()));
        assert_eq!(result.price_estimate_max, None);
    }

    #[test]
    fn should_extract_price_estimate_max_when_rule_present() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="a.jpg">
                <span id="est-max">1200</span>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: Some(text_rule("#est-max")),
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.price_estimate_max, Some("1200".to_string()));
    }

    #[test]
    fn should_extract_auction_start_and_end_when_rules_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(
            result.auction_start,
            Some("2025-06-01T10:00:00Z".to_string())
        );
        assert_eq!(result.auction_end, Some("2025-06-07T18:00:00Z".to_string()));
    }

    // -------------------------------------------------------------------------
    // Happy-path: optional fields absent (None rules)
    // -------------------------------------------------------------------------

    #[test]
    fn should_return_none_for_price_when_rule_is_absent() {
        let (html, schema) = minimal_schema(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.price, None);
    }

    #[test]
    fn should_return_empty_vec_for_description_when_rule_is_absent() {
        let (html, schema) = minimal_schema(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.description, Vec::<String>::new());
    }

    #[test]
    fn should_return_none_for_auction_fields_when_rules_are_absent() {
        let (html, schema) = minimal_schema(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.auction_start, None);
        assert_eq!(result.auction_end, None);
    }

    // -------------------------------------------------------------------------
    // Single-valued fields use only the first match
    // -------------------------------------------------------------------------

    #[test]
    fn should_use_first_match_for_title_when_multiple_h1_elements_present() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span>
                <h1>First Title</h1>
                <h1>Second Title</h1>
                <span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let (_, schema) = minimal_schema("<ignored>");
        // re-parse with the real HTML
        let schema = ProductCssSelectorSchema {
            title: text_rule("h1"),
            ..schema
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.title, "First Title");
    }

    #[test]
    fn should_use_first_match_for_price_when_multiple_elements_present() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
                <span class="price">€ 100</span>
                <span class="price">€ 200</span>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: Some(text_rule("span.price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.price, Some("€ 100".to_string()));
    }

    // -------------------------------------------------------------------------
    // Multi-valued fields keep all matches
    // -------------------------------------------------------------------------

    #[test]
    fn should_collect_all_description_fragments_for_all_cardinality() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
                <p class="desc">Fragment one</p>
                <p class="desc">Fragment two</p>
                <p class="desc">Fragment three</p>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: Some(text_rule_all("p.desc")),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(
            result.description,
            vec!["Fragment one", "Fragment two", "Fragment three"]
        );
    }

    #[test]
    fn should_collect_images_from_additional_selectors_in_order() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img class="main" src="main.jpg">
                <img class="thumb" src="thumb1.jpg">
                <img class="thumb" src="thumb2.jpg">
            </body></html>"#,
        );
        let images_rule = ExtractionRule {
            selector: CssSelector::from("img.main"),
            additional_selectors: vec![CssSelector::from("img.thumb")],
            extract: ExtractionKind::Attribute {
                name: HtmlAttributeName::from("src"),
            },
            cardinality: ExtractionCardinality::All,
        };
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: images_rule,
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.images, vec!["main.jpg", "thumb1.jpg", "thumb2.jpg"]);
    }

    // -------------------------------------------------------------------------
    // Error cases: mandatory fields
    // -------------------------------------------------------------------------

    #[test]
    fn should_return_err_shops_product_id_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body><h1>T</h1><span id="state">ok</span><img src="x.jpg"></body></html>"#,
        );
        let (_, schema) = minimal_schema("<ignored>");
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            ..schema
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::ShopsProductId(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_title_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><span id="state">ok</span><img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Title(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_state_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::State(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_images_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Images(_)),
            "unexpected variant: {err}"
        );
    }

    // -------------------------------------------------------------------------
    // Error cases: optional fields (rule present but selector fails)
    // -------------------------------------------------------------------------

    #[test]
    fn should_return_err_description_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: Some(text_rule_all("p.desc")),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Description(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_price_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: Some(text_rule("span.price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Price(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_price_estimate_min_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: Some(text_rule("#est-min")),
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::PriceEstimateMin(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_price_estimate_max_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: Some(text_rule("#est-max")),
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::PriceEstimateMax(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_auction_start_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: Some(attr_rule("time#auction-start", "datetime")),
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::AuctionStart(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_auction_end_when_rule_present_but_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: Some(attr_rule("time#auction-end", "datetime")),
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::AuctionEnd(_)),
            "unexpected variant: {err}"
        );
    }

    // -------------------------------------------------------------------------
    // Error cases: invalid selectors bubble up as the correct variant
    // -------------------------------------------------------------------------

    #[rstest]
    #[case("shops_product_id")]
    #[case("title")]
    #[case("state")]
    #[case("images")]
    fn should_return_invalid_selector_error_for_mandatory_field_when_selector_is_malformed(
        #[case] field: &str,
    ) {
        let html = Html::parse_document(product_html());
        let bad = text_rule("!!! invalid !!!");
        let good_id = text_rule("#product-id");
        let good_h1 = text_rule("h1");
        let good_state = text_rule("#state");
        let good_img = attr_rule_all("img", "src");

        let schema = match field {
            "shops_product_id" => ProductCssSelectorSchema {
                shops_product_id: bad,
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: good_img,
                auction_start: None,
                auction_end: None,
                default_currency: None,
            },
            "title" => ProductCssSelectorSchema {
                shops_product_id: good_id,
                title: bad,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: good_img,
                auction_start: None,
                auction_end: None,
                default_currency: None,
            },
            "state" => ProductCssSelectorSchema {
                shops_product_id: good_id,
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: bad,
                images: good_img,
                auction_start: None,
                auction_end: None,
                default_currency: None,
            },
            "images" => ProductCssSelectorSchema {
                shops_product_id: good_id,
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: bad,
                auction_start: None,
                auction_end: None,
                default_currency: None,
            },
            _ => unreachable!(),
        };

        let err = schema.apply(&html).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(field),
            "error message should name the field; got: {msg}"
        );
    }

    // -------------------------------------------------------------------------
    // Error message quality
    // -------------------------------------------------------------------------

    #[test]
    fn should_include_field_name_in_error_message_for_shops_product_id() {
        let html = Html::parse_document("<html><body></body></html>");
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#missing"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        // We only care that the error exists and mentions the field name.
        let err = schema.apply(&html).unwrap_err();
        assert!(err.to_string().contains("shops_product_id"), "{err}");
    }

    #[test]
    fn should_include_field_name_in_error_message_for_price() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: Some(text_rule(".price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(err.to_string().contains("price"), "{err}");
    }

    // -------------------------------------------------------------------------
    // Attribute extraction
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_image_srcs_via_attribute_rule_for_all_cardinality() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <div class="gallery">
                    <img src="a.jpg"><img src="b.jpg"><img src="c.jpg">
                </div>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("div.gallery img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.images, vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    #[test]
    fn should_return_err_images_when_img_element_missing_src_attribute() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img alt="no-src">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Images(_)),
            "unexpected variant: {err}"
        );
    }

    // -------------------------------------------------------------------------
    // Realistic full-page scenario
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_complete_product_when_full_schema_applied_to_realistic_page() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();

        assert_eq!(result.shops_product_id, "SKU-42");
        assert_eq!(result.title, "Biedermeier Chair");
        assert_eq!(
            result.description,
            vec!["A beautiful antique chair", "Circa 1830, walnut wood"]
        );
        assert_eq!(result.price, Some("€ 1.200".to_string()));
        assert_eq!(result.price_estimate_min, None);
        assert_eq!(result.price_estimate_max, None);
        assert_eq!(result.state, "In Stock");
        assert_eq!(
            result.images,
            vec!["/images/chair-front.jpg", "/images/chair-side.jpg"]
        );
        assert_eq!(
            result.auction_start,
            Some("2025-06-01T10:00:00Z".to_string())
        );
        assert_eq!(result.auction_end, Some("2025-06-07T18:00:00Z".to_string()));
    }

    #[test]
    fn should_extract_product_without_optional_fields_when_minimal_schema_applied() {
        let html = Html::parse_document(
            r#"<!DOCTYPE html><html><body>
                <span id="product-id">ITEM-1</span>
                <h1>Vintage Vase</h1>
                <span id="state">sold</span>
                <img src="vase.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        };
        let result = schema.apply(&html).unwrap();

        assert_eq!(result.shops_product_id, "ITEM-1");
        assert_eq!(result.title, "Vintage Vase");
        assert!(result.description.is_empty());
        assert_eq!(result.price, None);
        assert_eq!(result.price_estimate_min, None);
        assert_eq!(result.price_estimate_max, None);
        assert_eq!(result.state, "sold");
        assert_eq!(result.images, vec!["vase.jpg"]);
        assert_eq!(result.auction_start, None);
        assert_eq!(result.auction_end, None);
    }
}
