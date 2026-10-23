use crate::scraper::css_selector::rule::{
    ExtractionError, ExtractionRule, RuleExtraction, split_image_candidate_group,
};

use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use scraper::Html;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListingSourceProductSchema {
    pub listing_source_id: ListingSourceId,
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

    impl Dummy<Faker> for ListingSourceProductSchema {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ListingSourceProductSchema {
                listing_source_id: config.fake_with_rng(rng),
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
    description = "Schema of rules for extracting product information from a ListingSource website using CSS selectors.
    Each field represents a specific piece of information about the product, and the value is an ExtractionRule that defines how to extract that information from the HTML of the ListingSource website.
    The rules are intended to extract raw data from the HTML, not normalized data."
)]
pub struct ProductCssSelectorSchema {
    #[schemars(
        description = "ID of the product on the ListingSource website. Optional: leave null when the page has no stable product ID."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_listing_id: Option<ExtractionRule>,

    #[schemars(description = "Title of the product")]
    pub title: ExtractionRule,

    #[schemars(
        description = "Main product description/body content. May be fragmented across multiple nodes. Prefer the central product-description area for this item. Avoid shipping info, legal disclaimers, navigation text, marketing banners, and recommendation or related-products sections."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<ExtractionRule>,

    #[schemars(
        description = "Visible product price text. Prefer the actual price element for this product and extract human-visible text, not attributes. Struck-through descendants are dropped from extracted text, so a price element that also holds a struck-through comparison price yields the current price. Avoid page-level wrapper containers, totals for other products, shipping costs, and unrelated price widgets."
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
        description = "Availability state of the product. E.g. 'in stock', 'out of stock', 'preorder', 'add to cart', etc. Prioritize state sources in this order: (1) clear explicit state text such as 'available', 'sold', or 'out of stock'; (2) visible text from a product-specific add-to-cart or buy button; (3) visible text from other product-specific buttons that clearly indicate availability such as preorder, reserve, or sold-out actions. Prefer dedicated availability labels or visible button text over generic class names or whole script blobs. IMPORTANT: Never use price elements, image galleries, or generic layout wrappers as the state selector. Select only the availability/cart action element; exclude price text and avoid containers that combine action text with price."
    )]
    pub state: ExtractionRule,

    #[schemars(
        description = "ProductListing media URLs. May be fragmented across multiple gallery nodes. Prefer ImageUrl extraction for image/gallery nodes so the scraper can validate ordered full-size candidates. Candidate order is data-large_image, data-full, data-original, data-zoom-image, data-src, data-lazy-src, content, current href, parent href, largest picture/srcset candidate, then src. Avoid logos, icons, placeholders, sprites, unrelated thumbnails from navigation or recommendations, and thumbnail-only attributes such as 100x100 or 150x150 src values."
    )]
    pub images: ExtractionRule,

    #[schemars(
        description = "Crawler-only raw product attributes keyed by stable camelCase names from the configured raw attribute registry, such as rawShipment, rawCondition, rawMaterial, rawYear, rawPeriod, rawCategory, rawTags, rawMeasurements, rawOrigin, or rawArtistName. Use only for visible product-specific values that do not yet have normalized product fields. Extract raw values only; do not normalize or derive values."
    )]
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        default,
        alias = "rawAttributes"
    )]
    pub raw_attributes: BTreeMap<String, ExtractionRule>,
}

/// Errors that can occur when applying a [`ProductCssSelectorSchema`] to an HTML document.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ApplySchemaError {
    #[error("failed to extract `source_listing_id`: {0}")]
    SourceListingId(#[source] ExtractionError),

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

    #[error("failed to extract raw attribute `{field}`: {source}")]
    RawAttribute {
        field: String,
        #[source]
        source: ExtractionError,
    },
}

impl ProductCssSelectorSchema {
    /// Validates every configured rule deterministically (selector syntax and
    /// attribute names) without applying it to a page.
    pub(crate) fn validate_rules(&self) -> Result<(), ExtractionError> {
        [
            self.source_listing_id.as_ref(),
            Some(&self.title),
            self.description.as_ref(),
            self.price.as_ref(),
            self.price_estimate_min.as_ref(),
            self.price_estimate_max.as_ref(),
            Some(&self.state),
            Some(&self.images),
        ]
        .into_iter()
        .flatten()
        .chain(self.raw_attributes.values())
        .try_for_each(ExtractionRule::validate)
    }

    /// Validates the rules of every schema and returns the first invalid rule.
    pub(crate) fn validate_all(schemas: &[Self]) -> Result<(), ExtractionError> {
        Self::validate_all_indexed(schemas).map_err(|(_, error)| error)
    }

    /// Validates the rules of every schema and returns the first invalid rule together
    /// with the index of the schema that holds it.
    pub(crate) fn validate_all_indexed(schemas: &[Self]) -> Result<(), (usize, ExtractionError)> {
        schemas
            .iter()
            .enumerate()
            .try_for_each(|(index, schema)| schema.validate_rules().map_err(|error| (index, error)))
    }

    pub(crate) fn apply_image_url_candidate_groups(
        &self,
        html: &Html,
    ) -> Result<RuleExtraction, ExtractionError> {
        match self.images.apply_image_url_candidate_groups(html) {
            Ok(images) => Ok(images),
            Err(ExtractionError::NoElementMatched { .. })
                if image_rule_has_existing_empty_container(&self.images, html) =>
            {
                Ok(RuleExtraction::default())
            }
            Err(err) => Err(err),
        }
    }

    /// Apply all extraction rules in this schema to the given parsed HTML document,
    /// returning a [`RawExtractedProduct`] with the raw (non-normalised) values.
    ///
    /// Rules for optional fields are skipped (returning `None`) when the field itself
    /// is `None`. A present optional rule (`source_listing_id`, `description`, `price`,
    /// the price estimates and raw attributes) that matches nothing yields an empty
    /// value (`None` for the prices) and a per-field log diagnostic (warn for description and
    /// price, debug or trace otherwise) instead of a page error: one page without
    /// a description must not invalidate the schema. Only `title` and `state` fail
    /// closed on a miss; `images` fails closed unless the image container exists and
    /// holds no image (see [`Self::apply_image_url_candidate_groups`]). `images` skips
    /// gallery elements without a value (no image URL candidate, or no attribute of an
    /// attribute rule) and fails only when it skips every element it matched. Any other
    /// rule failure (invalid selector, missing attribute) still returns the
    /// corresponding [`ApplySchemaError`] variant immediately.
    ///
    /// For single-valued fields (`source_listing_id`, `title`, `state`) the first
    /// element of the extraction result is used. For multi-valued fields
    /// (`description`, `images`) all results are kept as a `Vec<String>`; `images` keeps
    /// the first image URL candidate of each gallery element, where [`Self::extract`]
    /// keeps the element's whole ordered group.
    ///
    /// Text separates lines at `<br>` and block boundaries, except in
    /// `source_listing_id`, which joins its text as written so a captured identity stays
    /// stable (see [`ExtractionRule::apply_as_identity`]).
    pub fn apply(&self, html: &Html) -> Result<RawExtractedProduct, ApplySchemaError> {
        let mut raw = self.extract(html)?.raw;
        raw.images = raw
            .images
            .into_iter()
            .map(|image| {
                split_image_candidate_group(&image)
                    .into_iter()
                    .next()
                    .unwrap_or(image.as_str())
                    .to_owned()
            })
            .collect();
        Ok(raw)
    }

    /// Applies this schema like [`Self::apply`], except that `images` keeps each gallery
    /// element's ordered image URL candidates as one group, which runtime image
    /// validation tries in order, and the result counts the `images` elements skipped for
    /// having no value. Runtime extraction and the schema review matrix both apply a
    /// schema this way, so each rule, the `images` rule included, runs once per page.
    pub(crate) fn extract(&self, html: &Html) -> Result<SchemaExtraction, ApplySchemaError> {
        let source_listing_id = match &self.source_listing_id {
            None => String::new(),
            Some(rule) => tolerate_optional_miss(
                "source_listing_id",
                rule.apply_as_identity(html),
                OptionalMissLog::Debug,
            )
            .map_err(ApplySchemaError::SourceListingId)?
            .into_iter()
            .next()
            .unwrap_or_default(),
        };

        let title = self
            .title
            .apply(html)
            .map_err(ApplySchemaError::Title)?
            .into_iter()
            .next()
            .unwrap_or_default();

        let description = match &self.description {
            None => vec![],
            Some(rule) => {
                tolerate_optional_miss("description", rule.apply(html), OptionalMissLog::Warn)
                    .map_err(ApplySchemaError::Description)?
            }
        };

        // A miss yields `None`, exactly like a schema without the rule, so plausibility
        // checks and the raw-values patch see "no price on this page".
        let price = match &self.price {
            None => None,
            Some(rule) => tolerate_optional_miss("price", rule.apply(html), OptionalMissLog::Warn)
                .map_err(ApplySchemaError::Price)?
                .into_iter()
                .next(),
        };

        let price_estimate_min = match &self.price_estimate_min {
            None => None,
            Some(rule) => tolerate_optional_miss(
                "price_estimate_min",
                rule.apply(html),
                OptionalMissLog::Debug,
            )
            .map_err(ApplySchemaError::PriceEstimateMin)?
            .into_iter()
            .next(),
        };

        let price_estimate_max = match &self.price_estimate_max {
            None => None,
            Some(rule) => tolerate_optional_miss(
                "price_estimate_max",
                rule.apply(html),
                OptionalMissLog::Debug,
            )
            .map_err(ApplySchemaError::PriceEstimateMax)?
            .into_iter()
            .next(),
        };

        let state = self
            .state
            .apply(html)
            .map_err(ApplySchemaError::State)?
            .into_iter()
            .next()
            .unwrap_or_default();

        let images = self
            .apply_image_url_candidate_groups(html)
            .map_err(ApplySchemaError::Images)?;

        let mut raw_attributes = BTreeMap::new();
        for (field, rule) in &self.raw_attributes {
            let values: Vec<String> =
                tolerate_optional_miss(field, rule.apply(html), OptionalMissLog::Trace)
                    .map_err(|source| ApplySchemaError::RawAttribute {
                        field: field.clone(),
                        source,
                    })?
                    .into_iter()
                    .filter(|value| !value.trim().is_empty())
                    .collect();
            if !values.is_empty() {
                raw_attributes.insert(field.clone(), values);
            }
        }

        Ok(SchemaExtraction {
            raw: RawExtractedProduct {
                source_listing_id,
                title,
                description,
                price,
                price_estimate_min,
                price_estimate_max,
                state,
                images: images.values,
                raw_attributes,
            },
            skipped_image_elements: images.skipped_elements,
        })
    }
}

/// Log level of an optional-rule miss.
///
/// A soft miss no longer fails the page, so selector drift on a cached schema only
/// shows up in the log: description and price misses are warnings so they stay
/// visible at the production log level, identity and estimate misses are debug
/// diagnostics, and raw attributes (absent on most pages by design) stay at trace.
#[derive(Clone, Copy)]
enum OptionalMissLog {
    Warn,
    Debug,
    Trace,
}

/// Turns a miss of an optional field's rule into an empty result.
///
/// The miss is still recorded as a per-field diagnostic so the scrape log explains
/// why the field is empty; the page itself keeps extracting.
fn tolerate_optional_miss(
    field: &str,
    extraction: Result<Vec<String>, ExtractionError>,
    miss_log: OptionalMissLog,
) -> Result<Vec<String>, ExtractionError> {
    match extraction {
        Ok(values) => Ok(values),
        Err(ExtractionError::NoElementMatched { selector }) => {
            // The message must be a literal in each arm: a `const` in message position
            // becomes a field named after the constant and leaves `message` empty.
            match miss_log {
                OptionalMissLog::Warn => tracing::warn!(
                    field,
                    selector = %selector,
                    "Optional schema rule matched nothing; leaving the field empty"
                ),
                OptionalMissLog::Debug => tracing::debug!(
                    field,
                    selector = %selector,
                    "Optional schema rule matched nothing; leaving the field empty"
                ),
                OptionalMissLog::Trace => tracing::trace!(
                    field,
                    selector = %selector,
                    "Optional schema rule matched nothing; leaving the field empty"
                ),
            }
            Ok(Vec::new())
        }
        Err(err) => Err(err),
    }
}

fn image_rule_has_existing_empty_container(rule: &ExtractionRule, html: &Html) -> bool {
    let mut found_container = false;

    for container_selector in std::iter::once(&rule.selector)
        .chain(rule.additional_selectors.iter())
        .flat_map(|selector| image_container_selectors(selector.as_ref()))
    {
        let Ok(container_selector) = scraper::Selector::parse(&container_selector) else {
            continue;
        };

        for container in html.select(&container_selector) {
            found_container = true;
            if element_has_image_like_evidence(&container) {
                return false;
            }
        }
    }

    found_container
}

fn image_container_selectors(selector: &str) -> Vec<String> {
    selector
        .split(',')
        .filter_map(|selector_part| {
            let selector_part = selector_part.trim();
            let image_start = selector_part.rfind(" img")?;
            let container = selector_part[..image_start].trim();
            if container.is_empty() {
                None
            } else {
                Some(container.to_owned())
            }
        })
        .collect()
}

fn element_has_image_like_evidence(element: &scraper::ElementRef<'_>) -> bool {
    let html = element.html().to_ascii_lowercase();
    [
        "<img",
        "<picture",
        "<source",
        " src=",
        " srcset=",
        " data-src=",
        " data-srcset=",
        " data-lazy=",
        " data-lazy-src=",
        " data-large_image=",
        " data-full=",
        " data-original=",
        " data-zoom-image=",
        " background-image",
    ]
    .iter()
    .any(|needle| html.contains(needle))
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawExtractedProduct {
    pub source_listing_id: String,
    pub title: String,
    pub description: Vec<String>,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub state: String,
    pub images: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}

/// What [`ProductCssSelectorSchema::extract`] captured from a page.
#[derive(Debug)]
pub(crate) struct SchemaExtraction {
    /// The raw values, with each gallery element's ordered image URL candidates as one
    /// `images` entry.
    pub(crate) raw: RawExtractedProduct,
    /// Matched `images` elements without a value (see
    /// [`RuleExtraction::skipped_elements`]).
    pub(crate) skipped_image_elements: usize,
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use scraper::Html;
    use std::collections::BTreeMap;

    use crate::scraper::css_selector::product_schema::{
        ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
    };
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionError, ExtractionKind, ExtractionRule,
        HtmlAttributeName, IMAGE_CANDIDATE_SEPARATOR,
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

    fn image_rule_all(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::ImageUrl,
            cardinality: ExtractionCardinality::All,
        }
    }

    /// Minimal valid schema covering only the mandatory fields.
    fn minimal_schema(html: &str) -> (Html, ProductCssSelectorSchema) {
        let parsed = Html::parse_document(html);
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
  <span class="seller">Kunstauktionshaus Leipzig | Schütte</span>
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: Some(text_rule_all("p.desc")),
            price: Some(text_rule("span.price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        }
    }

    // -------------------------------------------------------------------------
    // Schema / structured output (pre-existing tests, kept here)
    // -------------------------------------------------------------------------

    #[test]
    fn should_create_schema() {
        drop(schemars::schema_for!(ProductCssSelectorSchema));
    }

    #[test]
    fn should_deserialize_schema_when_raw_attributes_are_absent() {
        let raw = r##"{
            "source_listing_id": null,
            "title": {"selector": "h1", "additional_selectors": [], "type": "text", "cardinality": "first"},
            "description": null,
            "price": null,
            "price_estimate_min": null,
            "price_estimate_max": null,
            "state": {"selector": "#state", "additional_selectors": [], "type": "text", "cardinality": "first"},
            "images": {"selector": "img", "additional_selectors": [], "type": "attribute", "name": "src", "cardinality": "all"},
            "default_currency": null
        }"##;

        let schema: ProductCssSelectorSchema = serde_json::from_str(raw).unwrap();

        assert!(schema.raw_attributes.is_empty());
    }

    #[test]
    fn should_deserialize_schema_when_raw_attributes_use_camel_case_alias() {
        let raw = r##"{
            "source_listing_id": null,
            "title": {"selector": "h1", "additional_selectors": [], "type": "text", "cardinality": "first"},
            "description": null,
            "price": null,
            "price_estimate_min": null,
            "price_estimate_max": null,
            "state": {"selector": "#state", "additional_selectors": [], "type": "text", "cardinality": "first"},
            "images": {"selector": "img", "additional_selectors": [], "type": "attribute", "name": "src", "cardinality": "all"},
            "default_currency": null,
            "rawAttributes": {
                "rawShipment": {"selector": ".shipping", "additional_selectors": [], "type": "text", "cardinality": "all"}
            }
        }"##;

        let schema: ProductCssSelectorSchema = serde_json::from_str(raw).unwrap();

        assert!(schema.raw_attributes.contains_key("rawShipment"));
    }

    // -------------------------------------------------------------------------
    // Happy-path: mandatory fields
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_source_listing_id_when_element_present() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();
        assert_eq!(result.source_listing_id, "SKU-42");
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

    #[test]
    fn should_extract_all_images_even_when_images_rule_uses_default_first_cardinality() {
        let html = Html::parse_document(product_html());
        let mut schema = full_schema();
        schema.images.cardinality = ExtractionCardinality::First;

        let result = schema.apply(&html).unwrap();

        assert_eq!(
            result.images,
            vec!["/images/chair-front.jpg", "/images/chair-side.jpg"]
        );
    }

    #[test]
    fn should_allow_empty_images_when_product_page_has_no_image_markup() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">23175</span>
                <h1>Pair of Mid-Victorian Figured Mahogany Three-Drawer Jewellery Drawers.</h1>
                <span id="state">In Stock</span>
                <div id="wpgs-gallery" class="wcgs-woocommerce-product-gallery">
                    <div class="spswiper-wrapper"></div>
                </div>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule_all("#wpgs-gallery img, .wcgs-woocommerce-product-gallery img"),
            raw_attributes: Default::default(),
        };

        let result = schema.apply(&html).unwrap();

        assert!(result.images.is_empty());
    }

    #[test]
    fn should_return_err_images_when_missing_image_container_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">23175</span>
                <h1>Pair of Mid-Victorian Figured Mahogany Three-Drawer Jewellery Drawers.</h1>
                <span id="state">In Stock</span>
                <div id="wpgs-gallery" class="wcgs-woocommerce-product-gallery">
                    <div class="spswiper-wrapper"></div>
                </div>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule_all("#wrong-gallery img"),
            raw_attributes: Default::default(),
        };

        let err = schema.apply(&html).unwrap_err();

        assert!(
            matches!(err, ApplySchemaError::Images(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_images_when_container_has_image_like_markup_but_selector_misses() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">23175</span>
                <h1>Pair of Mid-Victorian Figured Mahogany Three-Drawer Jewellery Drawers.</h1>
                <span id="state">In Stock</span>
                <div id="wpgs-gallery" class="wcgs-woocommerce-product-gallery">
                    <picture>
                        <source srcset="/images/full-size.webp 1200w">
                    </picture>
                </div>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule_all("#wpgs-gallery img"),
            raw_attributes: Default::default(),
        };

        let err = schema.apply(&html).unwrap_err();

        assert!(
            matches!(err, ApplySchemaError::Images(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_extract_image_container_selectors_from_image_descendant_selectors() {
        assert_eq!(
            super::image_container_selectors(
                "#wpgs-gallery img, .wcgs-woocommerce-product-gallery img"
            ),
            vec!["#wpgs-gallery", ".wcgs-woocommerce-product-gallery"]
        );
        assert_eq!(
            super::image_container_selectors(".product-gallery picture img"),
            vec![".product-gallery picture"]
        );
        assert!(super::image_container_selectors("img.product-photo").is_empty());
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: Some(text_rule("#est-min")),
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: Some(text_rule("#est-max")),
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.price_estimate_max, Some("1200".to_string()));
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: Some(text_rule("span.price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: Some(text_rule_all("p.desc")),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: images_rule,
            raw_attributes: Default::default(),
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.images, vec!["main.jpg", "thumb1.jpg", "thumb2.jpg"]);
    }

    // -------------------------------------------------------------------------
    // Optional product ID
    // -------------------------------------------------------------------------

    #[test]
    fn should_return_empty_source_listing_id_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body><h1>T</h1><span id="state">ok</span><img src="x.jpg"></body></html>"#,
        );
        let (_, schema) = minimal_schema("<ignored>");
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            ..schema
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.source_listing_id, "");
    }

    #[test]
    fn should_return_empty_source_listing_id_when_rule_is_absent() {
        let html = Html::parse_document(product_html());
        let (_, schema) = minimal_schema("<ignored>");
        let schema = ProductCssSelectorSchema {
            source_listing_id: None,
            ..schema
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.source_listing_id, "");
    }

    /// An identity element whose text a `<br>` and a block child split, as a label such
    /// as "Ref:<br>11614" renders on a product page.
    mod identity_text_across_line_breaks {
        use super::*;

        const PAGE: &str = r#"<html><body><div id="product-id">Ref:<br>11614<p>A</p></div><h1>Pair of Windsor Chairs<br>English, c.1830</h1><span id="state">Available</span><img src="/a.jpg"></body></html>"#;

        #[test]
        fn should_join_the_identity_as_written_while_other_text_fields_separate_lines() {
            let (html, schema) = minimal_schema(PAGE);
            let schema = ProductCssSelectorSchema {
                description: Some(text_rule_all("#product-id")),
                ..schema
            };

            let result = schema.apply(&html).unwrap();

            assert_eq!(result.source_listing_id, "Ref:11614A");
            assert_eq!(result.description, vec!["Ref:\n11614\nA"]);
            assert_eq!(result.title, "Pair of Windsor Chairs\nEnglish, c.1830");
        }
    }

    // -------------------------------------------------------------------------
    // Error cases: mandatory fields
    // -------------------------------------------------------------------------

    #[test]
    fn should_return_err_title_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><span id="state">ok</span><img src="x.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::State(_)),
            "unexpected variant: {err}"
        );
    }

    #[test]
    fn should_return_err_images_when_bare_image_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(err, ApplySchemaError::Images(_)),
            "unexpected variant: {err}"
        );
    }

    // -------------------------------------------------------------------------
    // Optional fields fail softly (rule present but selector matches nothing)
    // -------------------------------------------------------------------------

    /// A schema with only the required rules configured, so each test below adds
    /// exactly the optional rule it exercises.
    fn required_only_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        }
    }

    /// A product page that carries every required element but no description,
    /// price or estimate markup.
    fn page_without_optional_elements() -> Html {
        Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="x.jpg">
            </body></html>"#,
        )
    }

    #[test]
    fn should_leave_description_empty_when_rule_present_but_selector_matches_nothing() {
        let html = page_without_optional_elements();
        let schema = ProductCssSelectorSchema {
            description: Some(text_rule_all("p.desc")),
            ..required_only_schema()
        };

        let result = schema
            .apply(&html)
            .unwrap_or_else(|err| panic!("optional miss must not fail the page: {err}"));

        assert_eq!(result.title, "T");
        assert_eq!(result.state, "ok");
        assert!(result.description.is_empty());
    }

    #[test]
    fn should_leave_price_empty_when_rule_present_but_selector_matches_nothing() {
        let html = page_without_optional_elements();
        let schema = ProductCssSelectorSchema {
            price: Some(text_rule("span.price")),
            ..required_only_schema()
        };

        let result = schema
            .apply(&html)
            .unwrap_or_else(|err| panic!("optional miss must not fail the page: {err}"));

        assert_eq!(result.title, "T");
        assert_eq!(result.price, None);
    }

    #[test]
    fn should_leave_price_estimates_empty_when_rules_present_but_selectors_match_nothing() {
        let html = page_without_optional_elements();
        let schema = ProductCssSelectorSchema {
            price_estimate_min: Some(text_rule("#est-min")),
            price_estimate_max: Some(text_rule("#est-max")),
            ..required_only_schema()
        };

        let result = schema
            .apply(&html)
            .unwrap_or_else(|err| panic!("optional miss must not fail the page: {err}"));

        assert_eq!(result.price_estimate_min, None);
        assert_eq!(result.price_estimate_max, None);
    }

    #[test]
    fn should_still_fail_closed_when_required_rule_matches_nothing_beside_optional_miss() {
        let html = page_without_optional_elements();
        let schema = ProductCssSelectorSchema {
            description: Some(text_rule_all("p.desc")),
            state: text_rule("#missing-state"),
            ..required_only_schema()
        };

        let err = schema.apply(&html).unwrap_err();

        assert!(
            matches!(err, ApplySchemaError::State(_)),
            "unexpected variant: {err}"
        );
    }

    // -------------------------------------------------------------------------
    // Error cases: invalid selectors bubble up as the correct variant
    // -------------------------------------------------------------------------

    #[rstest]
    #[case("source_listing_id")]
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
            "source_listing_id" => ProductCssSelectorSchema {
                source_listing_id: Some(bad),
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: good_img,
                raw_attributes: Default::default(),
            },
            "title" => ProductCssSelectorSchema {
                source_listing_id: Some(good_id),
                title: bad,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: good_img,
                raw_attributes: Default::default(),
            },
            "state" => ProductCssSelectorSchema {
                source_listing_id: Some(good_id),
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: bad,
                images: good_img,
                raw_attributes: Default::default(),
            },
            "images" => ProductCssSelectorSchema {
                source_listing_id: Some(good_id),
                title: good_h1,
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: good_state,
                images: bad,
                raw_attributes: Default::default(),
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
    fn should_include_field_name_in_error_message_for_source_listing_id() {
        let html = Html::parse_document(product_html());
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("!!! invalid !!!")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        // We only care that the error exists and mentions the field name.
        let err = schema.apply(&html).unwrap_err();
        assert!(err.to_string().contains("source_listing_id"), "{err}");
    }

    #[test]
    fn should_include_field_name_in_error_message_for_price() {
        let html = page_without_optional_elements();
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: Some(text_rule("!!! invalid !!!")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("div.gallery img", "src"),
            raw_attributes: Default::default(),
        };
        let result = schema.apply(&html).unwrap();
        assert_eq!(result.images, vec!["a.jpg", "b.jpg", "c.jpg"]);
    }

    /// The only matched `<img>` has no `src`, so no element yields a value, and no empty
    /// image container exists.
    #[test]
    fn should_return_err_images_when_img_element_missing_src_attribute() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img alt="no-src">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        let err = schema.apply(&html).unwrap_err();
        assert!(
            matches!(
                &err,
                ApplySchemaError::Images(ExtractionError::MissingAttribute { attribute, .. })
                    if attribute == "src"
            ),
            "unexpected variant: {err}"
        );
    }

    /// The `images` rule skips gallery elements without a value whatever its kind
    /// (#2099): an attribute rule skips an `<img>` without its attribute.
    #[test]
    fn should_skip_gallery_img_without_the_attribute_of_an_attribute_images_rule() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <div class="gallery">
                    <img src="a.jpg"><img alt="no-src"><img src="c.jpg">
                </div>
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("div.gallery img", "src"),
            raw_attributes: Default::default(),
        };

        let result = schema.apply(&html).unwrap();

        assert_eq!(result.images, vec!["a.jpg", "c.jpg"]);
    }

    #[test]
    fn should_extract_first_image_candidate_from_ordered_image_url_group() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="/thumb-100x100.jpg" data-large_image="/full-800x600.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: ExtractionRule {
                selector: CssSelector::from("img"),
                additional_selectors: vec![],
                extract: ExtractionKind::ImageUrl,
                cardinality: ExtractionCardinality::All,
            },
            raw_attributes: Default::default(),
        };

        let result = schema.apply(&html).unwrap();

        assert_eq!(result.images.len(), 1);
        assert_eq!(result.images[0], "/full-800x600.jpg");
    }

    #[test]
    fn should_keep_image_url_candidate_groups_and_count_skipped_elements_when_extracting() {
        let html = Html::parse_document(
            r#"<html><body>
                <span id="product-id">X</span><h1>T</h1><span id="state">ok</span>
                <img src="/thumb-100x100.jpg" data-large_image="/full-800x600.jpg">
                <img alt="placeholder">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            images: ExtractionRule {
                selector: CssSelector::from("img"),
                additional_selectors: vec![],
                extract: ExtractionKind::ImageUrl,
                cardinality: ExtractionCardinality::All,
            },
            ..required_only_schema()
        };

        let extraction = schema.extract(&html).unwrap();
        let applied = schema.apply(&html).unwrap();

        assert_eq!(
            extraction.raw.images,
            vec![format!(
                "/full-800x600.jpg{IMAGE_CANDIDATE_SEPARATOR}/thumb-100x100.jpg"
            )]
        );
        assert_eq!(extraction.skipped_image_elements, 1);
        assert_eq!(
            applied,
            RawExtractedProduct {
                images: vec!["/full-800x600.jpg".to_string()],
                ..extraction.raw
            }
        );
    }

    // -------------------------------------------------------------------------
    // Realistic full-page scenario
    // -------------------------------------------------------------------------

    #[test]
    fn should_extract_complete_product_when_full_schema_applied_to_realistic_page() {
        let html = Html::parse_document(product_html());
        let result = full_schema().apply(&html).unwrap();

        assert_eq!(result.source_listing_id, "SKU-42");
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
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            raw_attributes: Default::default(),
        };
        let result = schema.apply(&html).unwrap();

        assert_eq!(result.source_listing_id, "ITEM-1");
        assert_eq!(result.title, "Vintage Vase");
        assert!(result.description.is_empty());
        assert_eq!(result.price, None);
        assert_eq!(result.price_estimate_min, None);
        assert_eq!(result.price_estimate_max, None);
        assert_eq!(result.state, "sold");
        assert_eq!(result.images, vec!["vase.jpg"]);
    }

    #[test]
    fn should_extract_raw_attributes_when_rules_present() {
        let html = Html::parse_document(
            r#"<html><body>
              <h1>Chair</h1>
              <span id="state">Available</span>
              <img src="/chair.jpg">
              <p class="shipping">Shipping takes four to six weeks</p>
              <p class="condition">Good condition with restored polish</p>
              <p class="material">Walnut and brass</p>
              <p class="year">Circa 1830</p>
              <p class="period">Biedermeier period</p>
              <p class="category">Furniture / Seating</p>
              <p class="tags">antique, chair, seating</p>
              <p class="measurements">H 90 cm x W 45 cm x D 50 cm</p>
              <span class="height">90 cm</span>
              <span class="width">45 cm</span>
              <span class="depth">50 cm</span>
              <span class="diameter">30 cm</span>
              <span class="weight">12 kg</span>
              <p class="origin">Southern Germany</p>
              <span class="country">Germany</span>
              <span class="region">Bavaria</span>
              <span class="artist">Marta Maas-Fjetterstrom</span>
              <span class="maker">Chuanlhong Ceramic</span>
              <span class="designer">Eileen Gray</span>
              <span class="brand">Knoll</span>
              <span class="signature">Signed lower right</span>
              <p class="creator-note">School of Antwerp attribution</p>
              <span class="blank">   </span>
            </body></html>"#,
        );
        let mut raw_attributes = BTreeMap::new();
        raw_attributes.insert("rawShipment".to_string(), text_rule_all(".shipping"));
        raw_attributes.insert("rawShipmentNote".to_string(), text_rule_all(".blank"));
        raw_attributes.insert("rawCondition".to_string(), text_rule_all(".condition"));
        raw_attributes.insert("rawMaterial".to_string(), text_rule_all(".material"));
        raw_attributes.insert("rawYear".to_string(), text_rule_all(".year"));
        raw_attributes.insert("rawPeriod".to_string(), text_rule_all(".period"));
        raw_attributes.insert("rawCategoryPath".to_string(), text_rule_all(".category"));
        raw_attributes.insert("rawTags".to_string(), text_rule_all(".tags"));
        raw_attributes.insert(
            "rawMeasurements".to_string(),
            text_rule_all(".measurements"),
        );
        raw_attributes.insert("rawHeight".to_string(), text_rule_all(".height"));
        raw_attributes.insert("rawWidth".to_string(), text_rule_all(".width"));
        raw_attributes.insert("rawDepth".to_string(), text_rule_all(".depth"));
        raw_attributes.insert("rawDiameter".to_string(), text_rule_all(".diameter"));
        raw_attributes.insert("rawWeight".to_string(), text_rule_all(".weight"));
        raw_attributes.insert("rawOrigin".to_string(), text_rule_all(".origin"));
        raw_attributes.insert("rawCountry".to_string(), text_rule_all(".country"));
        raw_attributes.insert("rawRegion".to_string(), text_rule_all(".region"));
        raw_attributes.insert("rawArtistName".to_string(), text_rule_all(".artist"));
        raw_attributes.insert("rawMakerName".to_string(), text_rule_all(".maker"));
        raw_attributes.insert("rawDesignerName".to_string(), text_rule_all(".designer"));
        raw_attributes.insert("rawBrandName".to_string(), text_rule_all(".brand"));
        raw_attributes.insert("rawSignature".to_string(), text_rule_all(".signature"));
        raw_attributes.insert("rawCreatorNote".to_string(), text_rule_all(".creator-note"));
        raw_attributes.insert(
            "rawOriginNote".to_string(),
            text_rule_all(".missing-origin-note"),
        );
        let schema = ProductCssSelectorSchema {
            raw_attributes,
            ..minimal_schema("<ignored>").1
        };

        let result = schema.apply(&html).unwrap();

        assert_eq!(
            result.raw_attributes.get("rawShipment"),
            Some(&vec!["Shipping takes four to six weeks".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawCondition"),
            Some(&vec!["Good condition with restored polish".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawMaterial"),
            Some(&vec!["Walnut and brass".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawYear"),
            Some(&vec!["Circa 1830".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawPeriod"),
            Some(&vec!["Biedermeier period".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawCategoryPath"),
            Some(&vec!["Furniture / Seating".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawTags"),
            Some(&vec!["antique, chair, seating".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawMeasurements"),
            Some(&vec!["H 90 cm x W 45 cm x D 50 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawHeight"),
            Some(&vec!["90 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawWidth"),
            Some(&vec!["45 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawDepth"),
            Some(&vec!["50 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawDiameter"),
            Some(&vec!["30 cm".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawWeight"),
            Some(&vec!["12 kg".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawOrigin"),
            Some(&vec!["Southern Germany".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawCountry"),
            Some(&vec!["Germany".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawRegion"),
            Some(&vec!["Bavaria".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawArtistName"),
            Some(&vec!["Marta Maas-Fjetterstrom".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawMakerName"),
            Some(&vec!["Chuanlhong Ceramic".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawDesignerName"),
            Some(&vec!["Eileen Gray".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawBrandName"),
            Some(&vec!["Knoll".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawSignature"),
            Some(&vec!["Signed lower right".to_string()])
        );
        assert_eq!(
            result.raw_attributes.get("rawCreatorNote"),
            Some(&vec!["School of Antwerp attribution".to_string()])
        );
        assert!(!result.raw_attributes.contains_key("rawShipmentNote"));
        assert!(!result.raw_attributes.contains_key("rawOriginNote"));
    }

    #[test]
    fn should_ignore_missing_raw_attribute_rule_when_selector_matches_nothing() {
        let html = Html::parse_document(
            r#"<html><body>
              <h1>Chair</h1>
              <span id="state">Available</span>
              <img src="/chair.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            raw_attributes: [
                (
                    "rawMaterial".to_string(),
                    text_rule_all(".missing-material"),
                ),
                ("rawPeriod".to_string(), text_rule_all(".missing-period")),
                (
                    "rawArtistName".to_string(),
                    text_rule_all(".missing-artist"),
                ),
            ]
            .into(),
            ..minimal_schema("<ignored>").1
        };

        let result = schema.apply(&html).unwrap();

        assert!(result.raw_attributes.is_empty());
    }

    #[test]
    fn should_return_err_raw_attribute_when_selector_is_malformed() {
        let html = Html::parse_document(
            r#"<html><body>
              <h1>Chair</h1>
              <span id="state">Available</span>
              <img src="/chair.jpg">
            </body></html>"#,
        );
        let schema = ProductCssSelectorSchema {
            raw_attributes: [("rawMaterial".to_string(), text_rule_all("!!! invalid !!!"))].into(),
            ..minimal_schema("<ignored>").1
        };

        let err = schema.apply(&html).unwrap_err();

        assert!(
            matches!(err, ApplySchemaError::RawAttribute { ref field, .. } if field == "rawMaterial"),
            "unexpected variant: {err}"
        );
    }
}
