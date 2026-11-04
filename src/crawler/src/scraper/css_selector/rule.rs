use image_url::{image_url_candidate_group, is_gallery_placeholder};
use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use visible_text::{TextLayout, has_visible_text, visible_text};

pub(crate) use image_url::{
    element_has_image_like_evidence, has_known_image_extension, srcset_descriptor_width,
};

mod image_url;
mod json_ld;
mod visible_text;

pub use json_ld::{JsonLdOfferProperty, JsonLdProductProperty};

/// `property` of the top-level schema.org `Product` of the JSON-LD document `json`, as a
/// `json_ld_product` rule reads it.
pub(crate) fn json_ld_product_value(json: &str, property: JsonLdProductProperty) -> Option<String> {
    json_ld::product_value(json, property)
}

macro_rules! local_string_newtype {
    ($name:ident) => {
        #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
        #[derive(
            Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize, JsonSchema,
        )]
        pub struct $name(String);

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

local_string_newtype!(CssSelector);
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtractionRule {
    #[schemars(description = "Primary CSS-Selector for the value to extract")]
    pub selector: CssSelector,

    #[schemars(
        description = "Additional CSS-Selectors, evaluated after the primary selector in order. With cardinality `first` they are fallbacks: the first selector that yields a non-blank value wins and the later ones are not evaluated. With cardinality `all` they aggregate: the matches of every selector are collected in selector order, each element at most once; an element nested inside, or containing, an element the rule already extracted is skipped, so a container and its paragraphs are never captured together."
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_selectors: Vec<CssSelector>,

    #[schemars(description = "Kind value to extract with CSS-Selector")]
    #[serde(flatten)]
    pub extract: ExtractionKind,

    #[schemars(
        description = "How many values to extract: `first` (default) takes the first non-blank value of the first selector that yields one, `all` takes every matched element of every selector."
    )]
    #[serde(default)]
    pub cardinality: ExtractionCardinality,
}

local_string_newtype!(HtmlAttributeName);
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtractionKind {
    Text,
    Attribute {
        name: HtmlAttributeName,
    },
    /// The value of attribute `name`, or the visible text of the element when that
    /// attribute is missing or blank. One rule reads an `<input type="submit" value>`
    /// and a `<button>` label, or a text-less image's `alt` and a text label, as
    /// alternatives for the same field.
    AttributeOrText {
        name: HtmlAttributeName,
    },
    /// A property of the `offers` in the JSON-LD document that the matched
    /// `<script type="application/ld+json">` element holds. The first non-blank value in
    /// document order wins; an element whose JSON does not parse or holds no such offer
    /// property is blank.
    JsonLdOffer {
        property: JsonLdOfferProperty,
    },
    /// A property of the top-level schema.org `Product` in the JSON-LD document that the
    /// matched `<script type="application/ld+json">` element holds, such as the `productID`
    /// or `sku` that is in no element the page shows. The first non-blank value in document
    /// order wins; an element whose JSON does not parse or holds no such value is blank.
    JsonLdProduct {
        property: JsonLdProductProperty,
    },
    ImageUrl,
}

impl ExtractionKind {
    /// Whether a blank match is a miss like a text rule's, that is, whether the kind yields
    /// a value from what an element shows, not from an attribute it must carry.
    fn is_text_like(&self) -> bool {
        !matches!(self, Self::Attribute { .. } | Self::ImageUrl)
    }
}

pub(crate) const IMAGE_CANDIDATE_SEPARATOR: char = '\u{1f}';

pub(crate) fn split_image_candidate_group(raw: &str) -> Vec<&str> {
    raw.split(IMAGE_CANDIDATE_SEPARATOR)
        .filter(|candidate| !candidate.trim().is_empty())
        .collect()
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionCardinality {
    #[default]
    First,
    All,
}

/// The values a rule extracted from a page and the matched elements it skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuleExtraction {
    /// Empty only for an empty gallery: an `images` rule whose matched elements are all
    /// gallery placeholders, or, as the schema applies it, one whose selector matched
    /// nothing and names an existing image container without an image.
    pub(crate) values: Vec<String>,
    /// Matched elements without a value: no image URL candidate, or no attribute of an
    /// attribute rule. Only the `images` rule skips elements
    /// ([`ExtractionRule::apply_image_url_candidate_groups`]), because galleries mix image
    /// links with navigation or placeholder anchors such as `href="#"`; every other rule
    /// fails on an element without a value.
    pub(crate) skipped_elements: usize,
}

/// What cardinality `all` does with a matched element that holds no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ElementWithoutValue {
    /// Fails the rule.
    Fails,
    /// Skips and counts the element (see [`RuleExtraction::skipped_elements`]). A rule that
    /// skips every element it matched yields no value when each one is a gallery
    /// placeholder, an empty gallery, and fails otherwise.
    Skipped,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Clone, Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("invalid CSS selector `{selector}`: {reason}")]
    InvalidSelector { selector: String, reason: String },

    #[error("no element matched selector `{selector}` (or any additional selectors)")]
    NoElementMatched { selector: String },

    /// The rule found elements, but none holds a value: the page leaves the field empty
    /// where the selector expects it, while [`Self::NoElementMatched`] means the selector
    /// does not fit the page. `selector` is the first selector that matched an element.
    #[error(
        "every element matched by selector `{selector}` is empty, and no other selector of the rule yields a value"
    )]
    EmptyValue { selector: String },

    #[error("element matched by `{selector}` does not have attribute `{attribute}`")]
    MissingAttribute { selector: String, attribute: String },
}

impl ExtractionRule {
    /// The primary selector followed by the additional selectors, in the order the
    /// rule evaluates them.
    pub(crate) fn selectors(&self) -> impl Iterator<Item = &CssSelector> {
        std::iter::once(&self.selector).chain(self.additional_selectors.iter())
    }

    pub(crate) fn validate(&self) -> Result<(), ExtractionError> {
        for selector in self.selectors() {
            parse_selector(selector)?;
        }
        if let ExtractionKind::Attribute { name } | ExtractionKind::AttributeOrText { name } =
            &self.extract
            && name.as_ref().trim().is_empty()
        {
            return Err(ExtractionError::MissingAttribute {
                selector: self.selector.to_string(),
                attribute: String::new(),
            });
        }
        Ok(())
    }

    /// Apply this extraction rule to the given parsed HTML document.
    ///
    /// The primary selector and the additional selectors are evaluated in that order;
    /// the cardinality decides what the additional selectors mean:
    ///
    /// - **`First` cardinality**: the additional selectors are fallbacks. The first
    ///   selector that yields a non-blank value wins and the later ones are not
    ///   evaluated. Blank matches (empty after trimming) are skipped until a non-blank
    ///   value is found, for text and attribute rules alike.
    /// - **`All` cardinality**: the additional selectors aggregate. Every matched element
    ///   of every selector is taken, in selector order, each element at most once: an
    ///   element nested inside, or containing, an element the rule already extracted is
    ///   skipped, so a description container and its own paragraphs are never captured
    ///   together. An item that is empty after trimming is dropped.
    ///
    /// Text is laid out as [`TextLayout::Rendered`] describes: visually hidden text is
    /// dropped, `<br>` and block boundaries start a new line, table cells are separated by
    /// a space and every other whitespace run collapses to one space. Attribute values
    /// have their whitespace collapsed too.
    ///
    /// When no selector yields a non-blank value, a text rule with cardinality `first`,
    /// or a rule with cardinality `all`, that matched only empty elements returns
    /// `Err(EmptyValue)`, while an attribute rule with either cardinality returns the
    /// first blank value it matched, so the blank plausibility check reports it; a rule
    /// that matched no element returns `Err(NoElementMatched)`.
    pub fn apply(&self, html: &Html) -> Result<Vec<String>, ExtractionError> {
        self.extract(html, TextLayout::Rendered, ElementWithoutValue::Fails)
            .map(|extraction| extraction.values)
    }

    /// Applies this rule like [`Self::apply`], except that text joins its nodes as written,
    /// hidden text and whitespace included, and attribute values stay as written. A
    /// captured listing identity must not change with how text is laid out: a scrape whose
    /// identity differs from the one its listing was captured under is rejected as a
    /// conflict. As in [`Self::apply`], a blank item is skipped with either cardinality,
    /// judged on its value as written, so the first item is the first non-blank one.
    pub(crate) fn apply_as_identity(&self, html: &Html) -> Result<Vec<String>, ExtractionError> {
        self.extract(html, TextLayout::AsWritten, ElementWithoutValue::Fails)
            .map(|extraction| extraction.values)
    }

    /// Applies this rule as the `images` rule: with cardinality `all`, whatever its
    /// configured cardinality, so every gallery element yields its ordered image URL
    /// candidates as one group. A matched element without a value (no image URL
    /// candidate, or no attribute of an attribute rule) is skipped and counted. When the
    /// rule skips every element it matched, a gallery whose elements are all placeholders
    /// (see [`is_gallery_placeholder`]) is empty and yields no value; otherwise the
    /// selector does not fit the page, and the rule fails with the error of the first
    /// skipped element that is not a placeholder.
    pub(crate) fn apply_image_url_candidate_groups(
        &self,
        html: &Html,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut all_images_rule = self.clone();
        all_images_rule.cardinality = ExtractionCardinality::All;
        all_images_rule.extract(html, TextLayout::Rendered, ElementWithoutValue::Skipped)
    }

    fn extract(
        &self,
        html: &Html,
        layout: TextLayout,
        without_value: ElementWithoutValue,
    ) -> Result<RuleExtraction, ExtractionError> {
        match self.cardinality {
            ExtractionCardinality::First => self.extract_first(html, layout),
            ExtractionCardinality::All => self.extract_all(html, layout, without_value),
        }
    }

    /// Cardinality `first`: the first non-blank value of the first selector that yields
    /// one, in document order. The later selectors are fallbacks for pages where the
    /// earlier ones miss or match only blank elements. A matched element without a value
    /// (no attribute of an attribute rule, no image URL candidate) ends the search: it
    /// fails the rule when the rule reaches it before any value. When the rule finds only
    /// blank values, a text rule fails with `EmptyValue` and an attribute rule yields its
    /// first blank value.
    fn extract_first(
        &self,
        html: &Html,
        layout: TextLayout,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut first_blank = None;
        'selectors: for css_selector in self.selectors() {
            let parsed = parse_selector(css_selector)?;
            for element in html.select(&parsed) {
                let value =
                    match extract_from_element(&element, &self.extract, css_selector, layout) {
                        Ok(value) => value,
                        // After a blank value, the rule yields its first blank, as if every
                        // element it matched were blank.
                        Err(ExtractionError::MissingAttribute { .. }) if first_blank.is_some() => {
                            break 'selectors;
                        }
                        Err(error) => return Err(error),
                    };
                if !is_blank(&value) {
                    return Ok(single_value(value));
                }
                first_blank.get_or_insert((css_selector, value));
            }
        }

        match first_blank {
            None => Err(self.no_element_matched()),
            Some((selector, _)) if self.extract.is_text_like() => Err(empty_value(selector)),
            Some((_, value)) => Ok(single_value(value)),
        }
    }

    /// Cardinality `all`: every element of every selector, in selector order, each
    /// element at most once. An element nested inside, or containing, an element the
    /// rule already extracted holds the same content and is skipped.
    fn extract_all(
        &self,
        html: &Html,
        layout: TextLayout,
        without_value: ElementWithoutValue,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut extraction = RuleExtraction::default();
        let mut extracted = ExtractedElements::new();
        let mut first_non_placeholder_error = None;
        let mut first_selector_with_empty_elements = None;
        let mut first_blank_attribute = None;

        for css_selector in self.selectors() {
            let parsed = parse_selector(css_selector)?;
            for element in html.select(&parsed) {
                let overlapping_elements = element
                    .ancestors()
                    .chain(element.descendants())
                    .map(|node| node.id());
                if extracted.contains_any(overlapping_elements) {
                    continue;
                }
                let value = extract_from_element(&element, &self.extract, css_selector, layout);
                if let Ok(blank) = &value
                    && is_blank(blank)
                    && without_value == ElementWithoutValue::Fails
                    && matches!(self.extract, ExtractionKind::Attribute { .. })
                {
                    first_blank_attribute.get_or_insert_with(|| blank.clone());
                }
                match value.and_then(|value| non_empty(value, css_selector)) {
                    Ok(value) => {
                        extracted.insert(element.id());
                        extraction.values.push(value);
                    }
                    Err(ExtractionError::EmptyValue { .. })
                        if without_value == ElementWithoutValue::Fails =>
                    {
                        first_selector_with_empty_elements.get_or_insert(css_selector);
                    }
                    Err(error) if without_value == ElementWithoutValue::Skipped => {
                        extraction.skipped_elements += 1;
                        if first_non_placeholder_error.is_none()
                            && !is_gallery_placeholder(&element)
                        {
                            first_non_placeholder_error = Some(error);
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        if extraction.values.is_empty() {
            // An attribute rule whose matches are all blank yields its first blank, as a
            // `first` rule does, so the blank plausibility check reports the field instead
            // of it reading as unmatched.
            return match (first_non_placeholder_error, first_blank_attribute) {
                (Some(error), _) => Err(error),
                (None, _) if extraction.skipped_elements > 0 => Ok(extraction),
                (None, Some(blank)) => Ok(single_value(blank)),
                (None, None) => Err(match first_selector_with_empty_elements {
                    Some(selector) => empty_value(selector),
                    None => self.no_element_matched(),
                }),
            };
        }

        Ok(extraction)
    }

    fn no_element_matched(&self) -> ExtractionError {
        ExtractionError::NoElementMatched {
            selector: self.selector.to_string(),
        }
    }

    /// How many of the elements `selector` (this rule's primary or an additional
    /// selector) matches the rule can take a value from, as the schema review matrix
    /// counts matches. A text rule counts only elements with visible text: `first`
    /// cardinality skips a blank element, so a rule that matches only blank elements
    /// misses. Attribute and image URL rules count every element, blank or without a
    /// value: an attribute rule whose matches are all blank yields a blank value that the
    /// blank plausibility check reports rather than a miss, and the `images` rule's count
    /// includes the elements it skips.
    pub(crate) fn match_count(&self, html: &Html, selector: &Selector) -> usize {
        let elements = html.select(selector);
        if self.extract.is_text_like() {
            elements
                .filter(|element| element_shows_value(element, &self.extract))
                .count()
        } else {
            elements.count()
        }
    }

    /// How many of the elements `selector` matches hold no visible text, for a text rule:
    /// the elements [`Self::match_count`] leaves out. `0` for attribute and image URL
    /// rules, whose match count includes every element.
    pub(crate) fn empty_element_count(&self, html: &Html, selector: &Selector) -> usize {
        if self.extract.is_text_like() {
            html.select(selector)
                .filter(|element| !element_shows_value(element, &self.extract))
                .count()
        } else {
            0
        }
    }
}

fn single_value(value: String) -> RuleExtraction {
    RuleExtraction {
        values: vec![value],
        skipped_elements: 0,
    }
}

/// The elements a rule with cardinality `all` has extracted so far, by node id. The id
/// type is inferred from the elements: `scraper` does not re-export `ego_tree::NodeId`.
struct ExtractedElements<Id>(HashSet<Id>);

impl<Id: Eq + std::hash::Hash> ExtractedElements<Id> {
    fn new() -> Self {
        Self(HashSet::new())
    }

    fn insert(&mut self, id: Id) {
        self.0.insert(id);
    }

    /// Whether any of `ids` was extracted.
    fn contains_any(&self, mut ids: impl Iterator<Item = Id>) -> bool {
        !self.0.is_empty() && ids.any(|id| self.0.contains(&id))
    }
}

fn parse_selector(css_selector: &CssSelector) -> Result<Selector, ExtractionError> {
    Selector::parse(css_selector.as_ref()).map_err(|err| ExtractionError::InvalidSelector {
        selector: css_selector.to_string(),
        reason: format!("{err:?}"),
    })
}

/// `value`, unless it is empty after trimming: cardinality `all` drops such an item, as
/// cardinality `first` skips a blank value, so a list never holds a blank value. An
/// identity item that is not blank stays as written.
fn non_empty(value: String, css_selector: &CssSelector) -> Result<String, ExtractionError> {
    if is_blank(&value) {
        Err(empty_value(css_selector))
    } else {
        Ok(value)
    }
}

/// Whether an extracted value is blank: empty after trimming, non-breaking spaces
/// included. Text and attribute rules share this test.
fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

fn empty_value(css_selector: &CssSelector) -> ExtractionError {
    ExtractionError::EmptyValue {
        selector: css_selector.to_string(),
    }
}

/// The value of `element`. Text is laid out as `layout` says and trimmed. An attribute
/// value laid out as [`TextLayout::Rendered`] has each run of HTML whitespace collapsed
/// to one space and is trimmed; [`TextLayout::AsWritten`] keeps it as written.
fn extract_from_element(
    element: &scraper::ElementRef<'_>,
    kind: &ExtractionKind,
    selector_str: &str,
    layout: TextLayout,
) -> Result<String, ExtractionError> {
    match kind {
        ExtractionKind::Text => Ok(visible_text(element, layout).trim().to_owned()),
        ExtractionKind::Attribute { name } => {
            let attr_value = element.value().attr(name.as_ref()).ok_or_else(|| {
                ExtractionError::MissingAttribute {
                    selector: selector_str.to_owned(),
                    attribute: name.to_string(),
                }
            })?;
            Ok(match layout {
                TextLayout::Rendered => collapse_whitespace(attr_value).trim().to_owned(),
                TextLayout::AsWritten => attr_value.to_owned(),
            })
        }
        ExtractionKind::AttributeOrText { name } => {
            let attr_value = element
                .value()
                .attr(name.as_ref())
                .map(|value| match layout {
                    TextLayout::Rendered => collapse_whitespace(value).trim().to_owned(),
                    TextLayout::AsWritten => value.to_owned(),
                })
                .filter(|value| !is_blank(value));
            Ok(attr_value.unwrap_or_else(|| visible_text(element, layout).trim().to_owned()))
        }
        ExtractionKind::JsonLdOffer { property } => Ok(json_ld::offer_value(
            &element.text().collect::<String>(),
            *property,
        )
        .unwrap_or_default()),
        ExtractionKind::JsonLdProduct { property } => Ok(json_ld::product_value(
            &element.text().collect::<String>(),
            *property,
        )
        .unwrap_or_default()),
        ExtractionKind::ImageUrl => {
            image_url_candidate_group(element).ok_or_else(|| ExtractionError::MissingAttribute {
                selector: selector_str.to_owned(),
                attribute: "image URL candidate".to_owned(),
            })
        }
    }
}

/// Whether a text-like rule can take a non-blank value from `element`: visible text, an
/// attribute or text alternative, a JSON-LD offer property, or a JSON-LD product property.
fn element_shows_value(element: &scraper::ElementRef<'_>, kind: &ExtractionKind) -> bool {
    match kind {
        ExtractionKind::Text => has_visible_text(element),
        ExtractionKind::AttributeOrText { name } => {
            element
                .value()
                .attr(name.as_ref())
                .is_some_and(|value| !is_blank(value))
                || has_visible_text(element)
        }
        ExtractionKind::JsonLdOffer { property } => {
            json_ld::offer_value(&element.text().collect::<String>(), *property).is_some()
        }
        ExtractionKind::JsonLdProduct { property } => {
            json_ld::product_value(&element.text().collect::<String>(), *property).is_some()
        }
        ExtractionKind::Attribute { .. } | ExtractionKind::ImageUrl => true,
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ─── helpers ───────────────────────────────────────────────────────

    fn html(raw: &str) -> Html {
        Html::parse_fragment(raw)
    }

    fn rule(
        selector: &str,
        kind: ExtractionKind,
        cardinality: ExtractionCardinality,
    ) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: kind,
            cardinality,
        }
    }

    fn rule_with_additional(
        selector: &str,
        additional: Vec<&str>,
        kind: ExtractionKind,
        cardinality: ExtractionCardinality,
    ) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: additional.into_iter().map(CssSelector::from).collect(),
            extract: kind,
            cardinality,
        }
    }

    fn text_kind() -> ExtractionKind {
        ExtractionKind::Text
    }

    fn attr_kind(name: &str) -> ExtractionKind {
        ExtractionKind::Attribute {
            name: HtmlAttributeName::from(name),
        }
    }

    fn image_url_kind() -> ExtractionKind {
        ExtractionKind::ImageUrl
    }

    fn image_candidates(raw: &str) -> Vec<&str> {
        split_image_candidate_group(raw)
    }

    // ─── Text / First ──────────────────────────────────────────────────

    #[test]
    fn should_extract_text_when_single_element_matched_for_first_cardinality() {
        let doc = html("<div><h1>Hello World</h1></div>");
        let r = rule("h1", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Hello World"]);
    }

    #[test]
    fn should_extract_first_text_when_multiple_elements_matched_for_first_cardinality() {
        let doc = html("<ul><li>Alpha</li><li>Beta</li><li>Gamma</li></ul>");
        let r = rule("li", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Alpha"]);
    }

    #[test]
    fn should_extract_nested_text_when_element_has_children_for_text_kind() {
        let doc = html("<p>Hello, <em>beautiful</em> world!</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Hello, beautiful world!"]);
    }

    #[test]
    fn should_extract_trimmed_text_when_whitespace_around_content_for_text_kind() {
        let doc = html("<span>   padded   </span>");
        let r = rule("span", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["padded"]);
    }

    #[test]
    fn should_return_empty_value_when_only_empty_text_is_found_for_first_cardinality() {
        let doc = html("<div><br/></div>");
        let r = rule("br", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::EmptyValue { .. }));
    }

    #[test]
    fn should_return_empty_value_when_the_title_element_is_empty() {
        let doc =
            html(r#"<div class="item-title"><h1 class="item-title" itemprop="name"></h1></div>"#);
        let r = rule("h1.item-title", text_kind(), ExtractionCardinality::First);

        let err = r.apply(&doc).unwrap_err();

        assert_eq!(
            err.to_string(),
            "every element matched by selector `h1.item-title` is empty, and no other selector of the rule yields a value"
        );
    }

    #[test]
    fn should_report_the_first_selector_that_matched_empty_elements() {
        let doc = html(r#"<span class="name"> </span>"#);
        let r = rule_with_additional(
            "h1.missing",
            vec![".name", "h2.missing"],
            text_kind(),
            ExtractionCardinality::First,
        );

        let err = r.apply(&doc).unwrap_err();

        assert!(
            matches!(&err, ExtractionError::EmptyValue { selector } if selector == ".name"),
            "{err:?}"
        );
    }

    #[test]
    fn should_return_empty_value_for_identity_when_only_empty_text_is_found() {
        let doc = html(r#"<span id="product-id">   </span>"#);
        let r = rule("#product-id", text_kind(), ExtractionCardinality::First);

        let err = r.apply_as_identity(&doc).unwrap_err();

        assert!(matches!(err, ExtractionError::EmptyValue { .. }), "{err:?}");
    }

    #[test]
    fn should_skip_inline_style_when_container_holds_visible_text_for_text_kind() {
        let doc = html(
            "<div class=\"price\"><style>.price { color: red; } .hidden { display: none; }</style>
  Price on application  </div>",
        );
        let r = rule("div.price", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Price on application"]);
    }

    #[rstest]
    #[case::script("<script>function f() { return 1; }</script>")]
    #[case::template("<template><span>Hidden</span></template>")]
    #[case::noscript("<noscript><img alt=\"x\">Enable JS</noscript>")]
    #[case::nested_style("<span><style>p { margin: 0; }</style></span>")]
    fn should_skip_non_text_subtrees_when_extracting_text(#[case] hidden: &str) {
        let doc = html(&format!("<div>Visible {hidden}<em>text</em></div>"));
        let r = rule("div", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Visible text"]);
    }

    #[test]
    fn should_report_an_empty_value_when_selected_element_is_a_non_text_element() {
        let doc = html("<div><script>window.a = 1;</script><p>Visible</p></div>");
        let r = rule("script", text_kind(), ExtractionCardinality::All);
        assert!(matches!(
            r.apply(&doc),
            Err(ExtractionError::EmptyValue { selector }) if selector == "script"
        ));
    }

    #[rstest]
    #[case::del("del")]
    #[case::s("s")]
    #[case::strike("strike")]
    fn should_extract_current_price_when_old_price_is_struck_through(#[case] tag: &str) {
        let doc = html(&format!(
            "<span class=\"product-vip__price-value\"> <{tag}>$50.400,00</{tag}> $25.200,00 </span>"
        ));
        let r = rule(
            "span.product-vip__price-value",
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["$25.200,00"]);
    }

    #[test]
    fn should_extract_price_unchanged_when_nothing_is_struck_through() {
        let doc = html("<span class=\"product-vip__price-value\"> <b>$25.200,00</b> ARS </span>");
        let r = rule(
            "span.product-vip__price-value",
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["$25.200,00 ARS"]);
    }

    #[rstest]
    #[case::sold_title("<h1><s>Victorian chair</s></h1>", "h1", "Victorian chair")]
    #[case::sold_state(
        "<span id=\"state\"> <del>Available</del> </span>",
        "#state",
        "Available"
    )]
    fn should_keep_struck_through_text_when_it_is_the_whole_value(
        #[case] markup: &str,
        #[case] selector: &str,
        #[case] expected: &str,
    ) {
        let r = rule(selector, text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&html(markup)).unwrap(), vec![expected]);
    }

    #[test]
    fn should_close_gap_left_by_dropped_price_when_text_continues_after_it() {
        let doc = html("<span class=\"price\">$25 <del>$50</del> ARS</span>");
        let r = rule("span.price", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["$25 ARS"]);
    }

    #[test]
    fn should_drop_struck_through_words_from_description_prose() {
        let doc = html(
            "<div class=\"desc\"><p>Provenance: <s>unknown</s> Château</p><p>Oak,  two drawers</p></div>",
        );
        let r = rule(".desc p", text_kind(), ExtractionCardinality::All);
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["Provenance: Château", "Oak, two drawers"]
        );
    }

    #[test]
    fn should_extract_struck_through_text_when_rule_selects_struck_through_element() {
        let doc = html("<span><del>$50.400,00</del> $25.200,00</span>");
        let r = rule("del", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["$50.400,00"]);
    }

    // ─── Text / All ────────────────────────────────────────────────────

    #[test]
    fn should_extract_all_text_when_multiple_elements_for_all_cardinality() {
        let doc = html("<ul><li>Alpha</li><li>Beta</li><li>Gamma</li></ul>");
        let r = rule("li", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn should_extract_single_text_when_one_element_for_all_cardinality() {
        let doc = html("<p>Only one</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Only one"]);
    }

    #[test]
    fn should_extract_all_nested_text_when_elements_have_children_for_all_cardinality() {
        let doc = html("<div><p>Hello <b>bold</b></p><p>World <i>italic</i></p></div>");
        let r = rule("p", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Hello bold", "World italic"]);
    }

    // ─── Attribute / First ─────────────────────────────────────────────

    #[test]
    fn should_extract_attribute_when_element_has_attribute_for_first_cardinality() {
        let doc = html(r#"<a href="https://example.com">Link</a>"#);
        let r = rule("a", attr_kind("href"), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["https://example.com"]);
    }

    #[test]
    fn should_extract_first_attribute_when_multiple_elements_for_first_cardinality() {
        let doc = html(r#"<div><img src="a.png"/><img src="b.png"/></div>"#);
        let r = rule("img", attr_kind("src"), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["a.png"]);
    }

    #[test]
    fn should_extract_data_attribute_when_element_has_custom_attribute() {
        let doc = html(r#"<div data-price="42.50">ProductListing</div>"#);
        let r = rule("div", attr_kind("data-price"), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["42.50"]);
    }

    /// A `first` attribute rule skips blank values like a text rule does, and yields its
    /// first blank only when it finds no non-blank value.
    mod blank_first_attribute_values {
        use super::*;

        #[rstest]
        #[case::empty("")]
        #[case::spaces("   ")]
        #[case::html_whitespace("\n\t ")]
        #[case::non_breaking_space("\u{a0}")]
        fn should_fall_back_past_a_blank_first_attribute_to_the_next_element(#[case] blank: &str) {
            let doc = html(&format!(
                "<meta itemprop=\"price\" content=\"{blank}\"><meta itemprop=\"price\" content=\"450\">"
            ));
            let r = rule(
                "meta[itemprop='price']",
                attr_kind("content"),
                ExtractionCardinality::First,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec!["450"]);
        }

        #[test]
        fn should_yield_a_blank_value_when_every_matched_attribute_is_blank() {
            let doc = html("<meta content=\"  \"><meta content=\"\"><meta content=\"\u{a0}\">");
            let r = rule("meta", attr_kind("content"), ExtractionCardinality::First);

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec!["  "]);
        }

        #[test]
        fn should_fall_back_to_an_additional_selector_when_the_primary_matches_only_blanks() {
            let doc = html("<meta class=\"a\" content=\" \"><meta class=\"b\" content=\"Oak\">");
            let r = rule_with_additional(
                "meta.a",
                vec!["meta.b"],
                attr_kind("content"),
                ExtractionCardinality::First,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec!["Oak"]);
        }

        #[test]
        fn should_yield_the_first_blank_when_every_selector_matches_only_blanks() {
            let doc = html("<meta class=\"a\" content=\"  \"><meta class=\"b\" content=\"\">");
            let r = rule_with_additional(
                "meta.missing",
                vec!["meta.a", "meta.b"],
                attr_kind("content"),
                ExtractionCardinality::First,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec!["  "]);
        }

        /// The identity keeps its value as written, so the blank test, not the layout,
        /// decides that a non-breaking space is blank.
        #[rstest]
        #[case::space(" ")]
        #[case::non_breaking_space("\u{a0}")]
        fn should_fall_back_past_a_blank_identity_attribute_and_keep_the_value_as_written(
            #[case] blank: &str,
        ) {
            let doc = html(&format!(
                "<span data-sku=\"{blank}\"></span><span data-sku=\"AB  12\"></span>"
            ));
            let r = rule("span", attr_kind("data-sku"), ExtractionCardinality::First);

            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec!["AB  12"]);
        }

        #[test]
        fn should_yield_the_blank_when_an_element_without_the_attribute_follows_it() {
            let doc = html("<a href=\" \"></a><a>No href</a><a href=\"/oak\"></a>");
            let r = rule("a", attr_kind("href"), ExtractionCardinality::First);

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec![" "]);
        }

        #[test]
        fn should_yield_the_blank_when_a_fallback_selector_lacks_the_attribute() {
            let doc = html(
                "<a class=\"a\" href=\"\"></a><a class=\"b\">No href</a><a class=\"c\" href=\"/oak\"></a>",
            );
            let r = rule_with_additional(
                "a.a",
                vec!["a.b", "a.c"],
                attr_kind("href"),
                ExtractionCardinality::First,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
        }

        /// The element without the attribute ends the search across selectors, not only
        /// within its own, so the fallback selector is not evaluated.
        #[test]
        fn should_not_fall_back_past_an_element_without_the_attribute_after_a_blank() {
            let doc = html(
                "<a class=\"a\" href=\" \"></a><a class=\"a\">No href</a><a class=\"b\" href=\"/oak\"></a>",
            );
            let r = rule_with_additional(
                "a.a",
                vec!["a.b"],
                attr_kind("href"),
                ExtractionCardinality::First,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec![" "]);
        }

        #[test]
        fn should_fail_on_an_element_without_the_attribute_before_any_value() {
            let doc = html("<a>No href</a><a href=\" \"></a><a href=\"/oak\"></a>");
            let r = rule("a", attr_kind("href"), ExtractionCardinality::First);

            assert!(matches!(
                r.apply(&doc),
                Err(ExtractionError::MissingAttribute { attribute, .. }) if attribute == "href"
            ));
        }

        #[test]
        fn should_count_blank_attribute_matches() {
            let doc = html("<meta content=\"\"><meta content=\" \"><meta content=\"Oak\">");
            let r = rule("meta", attr_kind("content"), ExtractionCardinality::First);
            let selector = Selector::parse("meta").unwrap();

            assert_eq!(r.match_count(&doc, &selector), 3);
            assert_eq!(r.empty_element_count(&doc, &selector), 0);
        }
    }

    // ─── Attribute / All ───────────────────────────────────────────────

    #[test]
    fn should_extract_all_attributes_when_multiple_elements_for_all_cardinality() {
        let doc = html(r#"<div><img src="a.png"/><img src="b.png"/><img src="c.png"/></div>"#);
        let r = rule("img", attr_kind("src"), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["a.png", "b.png", "c.png"]);
    }

    // ─── Additional selectors: fallback for `first`, aggregation for `all` ──

    #[test]
    fn should_take_only_the_primary_value_when_it_matches_for_first_cardinality() {
        let doc = html("<h1>Primary</h1><h2>Additional</h2>");
        let r = rule_with_additional("h1", vec!["h2"], text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Primary"]);
    }

    #[test]
    fn should_fall_back_to_the_first_additional_selector_that_matches_for_first_cardinality() {
        let doc = html("<h1></h1><h3>Third</h3><h4>Fourth</h4>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3", "h4"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["Third"]);
    }

    #[test]
    fn should_aggregate_results_from_all_three_selectors_in_order_for_all_cardinality() {
        let doc = html("<h1>First</h1><h2>Second</h2><h3>Third</h3>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["First", "Second", "Third"]);
    }

    #[test]
    fn should_skip_non_matching_additional_selector_without_error_for_all_cardinality() {
        let doc = html("<h1>Primary</h1><h3>Third</h3>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["Primary", "Third"]);
    }

    #[test]
    fn should_return_results_only_from_additional_when_primary_does_not_match() {
        let doc = html("<h2>Additional only</h2>");
        let r = rule_with_additional("h1", vec!["h2"], text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Additional only"]);
    }

    #[test]
    fn should_skip_empty_text_match_and_take_later_non_empty_match_for_first_cardinality() {
        let doc = html(
            r#"<div class="price"></div><div class="price">
                $1,125
            </div>"#,
        );
        let r = rule(".price", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["$1,125"]);
    }

    #[test]
    fn should_skip_empty_text_matches_from_primary_and_use_additional_text_selector() {
        let doc = html(r#"<div class="price"></div><div class="fallback">$1,125</div>"#);
        let r = rule_with_additional(
            ".price",
            vec![".fallback"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["$1,125"]);
    }

    #[test]
    fn should_aggregate_attribute_results_from_multiple_selectors() {
        let doc =
            html(r#"<a href="link1.html">L1</a><img src="img1.png"/><a href="link2.html">L2</a>"#);
        let r = rule_with_additional(
            "a",
            vec!["img"],
            attr_kind("href"),
            ExtractionCardinality::All,
        );
        // "a" matches both <a> elements → href extracted;
        // "img" has no href → MissingAttribute error
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
    }

    #[test]
    fn should_aggregate_all_elements_across_selectors_for_all_cardinality() {
        let doc = html(
            r#"<div class="gallery"><img src="a.png"/><img src="b.png"/></div><div class="extra"><img src="c.png"/></div>"#,
        );
        let r = rule_with_additional(
            ".gallery img",
            vec![".extra img"],
            attr_kind("src"),
            ExtractionCardinality::All,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["a.png", "b.png", "c.png"]);
    }

    #[test]
    fn should_take_the_first_element_of_the_primary_selector_only_for_first_cardinality() {
        let doc = html(
            r#"<div class="gallery"><img src="a.png"/><img src="b.png"/></div><div class="extra"><img src="c.png"/><img src="d.png"/></div>"#,
        );
        let r = rule_with_additional(
            ".gallery img",
            vec![".extra img"],
            attr_kind("src"),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["a.png"]);
    }

    #[test]
    fn should_aggregate_text_across_selectors_with_all_cardinality() {
        let doc = html("<ul><li>UL1</li><li>UL2</li></ul><ol><li>OL1</li></ol>");
        let r = rule_with_additional(
            "ul > li",
            vec!["ol > li"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["UL1", "UL2", "OL1"]);
    }

    #[test]
    fn should_preserve_order_across_selectors_not_document_order() {
        // Even though <h2> appears before <h3> in the document,
        // if additional_selectors lists "h3" before "h2", results
        // follow selector order.
        let doc = html("<h2>Two</h2><h3>Three</h3>");
        let r = rule_with_additional("h3", vec!["h2"], text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Three", "Two"]);
    }

    // ─── Overlapping selectors never capture the same content twice ────

    /// A description block the way Wick and Van Leest render it: a container with a
    /// title and paragraphs, selected as the container and again as its paragraphs.
    const DESCRIPTION_HTML: &str = r#"<div class="desc"><h2>Details</h2><p>Walnut.</p><p>Circa 1830.</p></div><p class="note">Restored.</p>"#;

    #[test]
    fn should_keep_the_container_only_when_its_paragraphs_are_also_selected_for_all_cardinality() {
        let doc = html(DESCRIPTION_HTML);
        let r = rule_with_additional(
            ".desc",
            vec![".desc p", ".desc h2", ".note"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["Details\nWalnut.\nCirca 1830.", "Restored."]
        );
    }

    #[test]
    fn should_skip_a_container_of_already_extracted_paragraphs_for_all_cardinality() {
        let doc = html(DESCRIPTION_HTML);
        let r = rule_with_additional(
            ".desc p",
            vec![".desc", ".note"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["Walnut.", "Circa 1830.", "Restored."]
        );
    }

    #[test]
    fn should_extract_an_element_once_when_two_selectors_match_it_for_all_cardinality() {
        let doc = html(DESCRIPTION_HTML);
        let r = rule_with_additional(
            ".desc p",
            vec!["div p", "p"],
            text_kind(),
            ExtractionCardinality::All,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["Walnut.", "Circa 1830.", "Restored."]
        );
    }

    #[test]
    fn should_skip_nested_matches_of_one_selector_for_all_cardinality() {
        let doc =
            html(r#"<div class="a"><div class="a">Inner</div></div><div class="a">Other</div>"#);
        let r = rule("div.a", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Inner", "Other"]);
    }

    #[test]
    fn should_keep_identical_text_of_separate_elements_for_all_cardinality() {
        let doc = html("<p>Walnut.</p><p>Walnut.</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Walnut.", "Walnut."]);
    }

    #[test]
    fn should_take_one_value_when_selectors_overlap_for_first_cardinality() {
        let doc = html(DESCRIPTION_HTML);
        let r = rule_with_additional(
            ".desc",
            vec![".desc p"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["Details\nWalnut.\nCirca 1830."]
        );
    }

    /// Van Leest's gallery, selected once as its links and again as the images inside
    /// them: every image came out twice.
    #[test]
    fn should_extract_a_gallery_image_once_when_its_link_and_its_img_are_both_selected() {
        let doc = html(
            r#"<div class="gallery"><a href="/full/a.jpg"><img src="/thumb/a.jpg"></a><a href="/full/b.jpg"><img src="/thumb/b.jpg"></a></div>"#,
        );
        let r = rule_with_additional(
            ".gallery a",
            vec![".gallery img"],
            image_url_kind(),
            ExtractionCardinality::All,
        );

        let images = r.apply_image_url_candidate_groups(&doc).unwrap();

        assert_eq!(
            images
                .values
                .iter()
                .map(|group| image_candidates(group)[0])
                .collect::<Vec<_>>(),
            vec!["/full/a.jpg", "/full/b.jpg"]
        );
        assert_eq!(images.skipped_elements, 0);
    }

    #[test]
    fn should_return_single_result_when_only_additional_matches_with_first_cardinality() {
        let doc = html(r#"<img src="only.png"/>"#);
        let r = rule_with_additional(
            "a",
            vec!["img"],
            attr_kind("src"),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["only.png"]);
    }

    // ─── Error: NoElementMatched ───────────────────────────────────────

    #[test]
    fn should_return_no_element_matched_when_selector_matches_nothing() {
        let doc = html("<p>Hello</p>");
        let r = rule("h1", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
    }

    #[test]
    fn should_return_no_element_matched_when_all_selectors_match_nothing() {
        let doc = html("<p>Hello</p>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
    }

    #[test]
    fn should_report_primary_selector_in_no_element_matched_error() {
        let doc = html("<p>Hello</p>");
        let r = rule_with_additional(
            "h1.special",
            vec!["h2"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        match err {
            ExtractionError::NoElementMatched { selector } => {
                assert_eq!(selector, "h1.special");
            }
            _ => panic!("expected NoElementMatched"),
        }
    }

    #[test]
    fn should_return_no_element_matched_when_empty_html_fragment() {
        let doc = html("");
        let r = rule("div", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
    }

    #[test]
    fn should_return_no_element_matched_for_all_cardinality_when_nothing_matches() {
        let doc = html("<p>Hello</p>");
        let r = rule("span", text_kind(), ExtractionCardinality::All);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
    }

    #[test]
    fn should_return_empty_value_when_all_text_matches_are_empty() {
        let doc = html(r#"<div class="price"></div><div class="fallback">   </div>"#);
        let r = rule_with_additional(
            ".price",
            vec![".fallback"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(
            matches!(&err, ExtractionError::EmptyValue { selector } if selector == ".price"),
            "{err:?}"
        );
    }

    // ─── Error: InvalidSelector ────────────────────────────────────────

    #[test]
    fn should_return_invalid_selector_when_primary_selector_is_malformed() {
        let doc = html("<p>Hello</p>");
        let r = rule("[[[invalid", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::InvalidSelector { .. }));
    }

    #[test]
    fn should_report_selector_string_in_invalid_selector_error() {
        let doc = html("<p>Hello</p>");
        let r = rule("!!!", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        match err {
            ExtractionError::InvalidSelector { selector, .. } => {
                assert_eq!(selector, "!!!");
            }
            _ => panic!("expected InvalidSelector"),
        }
    }

    #[test]
    fn should_return_invalid_selector_when_additional_is_malformed() {
        let doc = html("<h1></h1>");
        let r = rule_with_additional(
            "h1",
            vec!["[[[bad"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::InvalidSelector { .. }));
    }

    /// A matching primary selector does not evaluate the fallbacks, so `validate` is
    /// what rejects a malformed additional selector before a schema is persisted.
    #[test]
    fn should_reject_a_malformed_additional_selector_on_validation() {
        let r = rule_with_additional(
            "h1",
            vec!["[[[bad"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert!(matches!(
            r.validate().unwrap_err(),
            ExtractionError::InvalidSelector { .. }
        ));
    }

    #[test]
    fn should_return_invalid_selector_when_empty_selector_string() {
        let doc = html("<p>Hello</p>");
        let r = rule("", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::InvalidSelector { .. }));
    }

    #[test]
    fn should_return_invalid_selector_for_malformed_additional_even_when_primary_has_no_match() {
        let doc = html("<p>Hello</p>");
        let r = rule_with_additional(
            "h1",
            vec!["[[[bad"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::InvalidSelector { .. }));
    }

    // ─── Error: MissingAttribute ───────────────────────────────────────

    #[test]
    fn should_return_missing_attribute_when_element_lacks_requested_attribute() {
        let doc = html(r#"<a>No href</a>"#);
        let r = rule("a", attr_kind("href"), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
    }

    #[test]
    fn should_report_attribute_name_in_missing_attribute_error() {
        let doc = html(r#"<img alt="pic"/>"#);
        let r = rule("img", attr_kind("src"), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        match err {
            ExtractionError::MissingAttribute {
                attribute,
                selector,
            } => {
                assert_eq!(attribute, "src");
                assert_eq!(selector, "img");
            }
            _ => panic!("expected MissingAttribute"),
        }
    }

    #[test]
    fn should_return_missing_attribute_when_first_of_all_lacks_attribute() {
        let doc = html(r#"<div><span>no attr</span><span data-x="y">has attr</span></div>"#);
        let r = rule("span", attr_kind("data-x"), ExtractionCardinality::All);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
    }

    #[test]
    fn should_return_missing_attribute_when_additional_selector_element_lacks_attribute() {
        let doc = html(r#"<span>No href</span>"#);
        let r = rule_with_additional(
            "a",
            vec!["span"],
            attr_kind("href"),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
    }

    // ─── Complex / realistic selectors ─────────────────────────────────

    #[test]
    fn should_extract_text_when_using_class_selector() {
        let doc = html(r#"<div class="price">€42.00</div><div class="title">Widget</div>"#);
        let r = rule(".price", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["€42.00"]);
    }

    #[test]
    fn should_extract_text_when_using_id_selector() {
        let doc = html(r#"<span id="product-title">My ProductListing</span>"#);
        let r = rule("#product-title", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["My ProductListing"]);
    }

    #[test]
    fn should_extract_attribute_when_using_descendant_selector() {
        let doc =
            html(r#"<div class="gallery"><a href="img1.jpg">1</a><a href="img2.jpg">2</a></div>"#);
        let r = rule(".gallery a", attr_kind("href"), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["img1.jpg", "img2.jpg"]);
    }

    #[test]
    fn should_extract_text_when_using_attribute_selector() {
        let doc =
            html(r#"<input type="text" value="hello"/><input type="hidden" value="secret"/>"#);
        let r = rule(
            r#"input[type="hidden"]"#,
            attr_kind("value"),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["secret"]);
    }

    #[test]
    fn should_extract_text_when_using_child_combinator_selector() {
        let doc = html("<div><ul><li>Direct</li></ul><li>Not direct</li></div>");
        let r = rule("ul > li", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Direct"]);
    }

    // ─── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn should_extract_empty_attribute_value_when_attribute_is_present_but_empty() {
        let doc = html(r#"<div data-value="">Content</div>"#);
        let r = rule("div", attr_kind("data-value"), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec![""]);
    }

    #[test]
    fn should_extract_text_with_special_characters() {
        let doc = html("<p>&lt;script&gt;alert('xss')&lt;/script&gt;</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::First);
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["<script>alert('xss')</script>"]
        );
    }

    #[test]
    fn should_extract_text_with_unicode_content() {
        let doc = html("<p>日本語テキスト</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["日本語テキスト"]);
    }

    #[test]
    fn should_extract_attribute_with_unicode_value() {
        let doc = html(r#"<a href="https://example.com/café">Link</a>"#);
        let r = rule("a", attr_kind("href"), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["https://example.com/café"]);
    }

    #[test]
    fn should_extract_text_from_deeply_nested_structure() {
        let doc = html("<div><section><article><p><span>Deep</span></p></article></section></div>");
        let r = rule(
            "div section article p span",
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["Deep"]);
    }

    #[test]
    fn should_drop_empty_items_for_all_cardinality() {
        let doc = html("<ul><li>  </li><li></li><li>Real</li><li>\n\t</li></ul>");
        let r = rule("li", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Real"]);
    }

    mod empty_items {
        use super::*;

        #[test]
        fn should_report_an_empty_value_when_every_match_is_empty_for_all_cardinality() {
            let doc = html("<div class=\"note\"> </div><p class=\"desc\"><br></p>");
            let r = rule_with_additional(
                ".missing",
                vec![".desc", ".note"],
                text_kind(),
                ExtractionCardinality::All,
            );

            assert!(matches!(
                r.apply(&doc),
                Err(ExtractionError::EmptyValue { selector }) if selector == ".desc"
            ));
        }

        /// An empty element is not extracted, so it does not hide a container or
        /// descendant that a later selector of the rule matches.
        #[test]
        fn should_extract_a_containing_element_when_the_nested_match_was_empty() {
            let doc = html("<div class=\"desc\"><p class=\"intro\"> </p><p>Oak bureau</p></div>");
            let r = rule_with_additional(
                ".intro",
                vec![".desc"],
                text_kind(),
                ExtractionCardinality::All,
            );

            assert_eq!(r.apply(&doc).unwrap(), vec!["Oak bureau"]);
        }

        #[test]
        fn should_yield_the_first_blank_when_every_attribute_match_of_an_all_rule_is_blank() {
            let doc = html("<meta content=\"  \"><meta content=\"\">");
            let r = rule("meta", attr_kind("content"), ExtractionCardinality::All);

            assert_eq!(r.apply(&doc).unwrap(), vec![""]);
        }

        #[test]
        fn should_still_fail_an_all_text_rule_whose_matches_are_all_empty() {
            let doc = html("<p> </p><p></p>");
            let r = rule("p", text_kind(), ExtractionCardinality::All);

            assert!(matches!(
                r.apply(&doc),
                Err(ExtractionError::EmptyValue { .. })
            ));
        }

        #[test]
        fn should_drop_blank_attribute_values_for_all_cardinality() {
            let doc = html("<meta content=\"  \"><meta content=\"Oak\"><meta content=\"\">");
            let r = rule("meta", attr_kind("content"), ExtractionCardinality::All);

            assert_eq!(r.apply(&doc).unwrap(), vec!["Oak"]);
        }

        #[test]
        fn should_skip_and_count_an_image_element_with_a_blank_attribute() {
            let doc = html(
                "<a class=\"g\" href=\"https://shop.example/oak.jpg\"></a><a class=\"g\" href=\" \"></a>",
            );
            let r = rule("a.g", attr_kind("href"), ExtractionCardinality::All);

            let extraction = r.apply_image_url_candidate_groups(&doc).unwrap();

            assert_eq!(extraction.values, vec!["https://shop.example/oak.jpg"]);
            assert_eq!(extraction.skipped_elements, 1);
        }
    }

    mod whitespace_in_values {
        use super::*;

        #[test]
        fn should_collapse_whitespace_in_attribute_values() {
            let doc = html("<meta content=\"\n    Antique Silver\n      Early English\tSpoons \">");
            let r = rule("meta", attr_kind("content"), ExtractionCardinality::First);

            assert_eq!(
                r.apply(&doc).unwrap(),
                vec!["Antique Silver Early English Spoons"]
            );
        }

        /// An `all` identity rule drops a blank item as a `first` one skips it, so both take
        /// the first non-blank identity, and keeps the other items as written.
        #[rstest]
        #[case::text(
            text_kind(),
            "<span class=\"sku\"> </span><span class=\"sku\">AB  12</span>"
        )]
        #[case::attribute(
            attr_kind("data-sku"),
            "<span class=\"sku\" data-sku=\"\u{a0}\"></span><span class=\"sku\" data-sku=\"AB  12\"></span>"
        )]
        fn should_drop_a_leading_blank_identity_item_of_an_all_rule(
            #[case] kind: ExtractionKind,
            #[case] page: &str,
        ) {
            let doc = html(page);
            let all = rule(".sku", kind.clone(), ExtractionCardinality::All);
            let first = rule(".sku", kind, ExtractionCardinality::First);

            assert_eq!(all.apply_as_identity(&doc).unwrap(), vec!["AB  12"]);
            assert_eq!(first.apply_as_identity(&doc).unwrap(), vec!["AB  12"]);
        }

        #[test]
        fn should_yield_the_first_blank_when_every_identity_item_of_an_all_attribute_rule_is_blank()
        {
            let doc = html(
                "<span class=\"sku\" data-sku=\" \"></span><span class=\"sku\" data-sku=\"\"></span>",
            );
            let r = rule(".sku", attr_kind("data-sku"), ExtractionCardinality::All);

            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec![" "]);
        }

        #[test]
        fn should_keep_identity_attribute_values_as_written() {
            let doc = html("<span data-sku=\"AB  12\"></span>");
            let r = rule("span", attr_kind("data-sku"), ExtractionCardinality::First);

            assert_eq!(r.apply_as_identity(&doc).unwrap(), vec!["AB  12"]);
        }

        #[test]
        fn should_collapse_indentation_in_multi_match_text() {
            let doc = html(
                "<ul class=\"crumbs\">\n  <li>\n    Antique Silver\n  </li>\n  <li>\n    Early English\n    Antique Spoons\n  </li>\n</ul>",
            );
            let r = rule(".crumbs li", text_kind(), ExtractionCardinality::All);

            assert_eq!(
                r.apply(&doc).unwrap(),
                vec!["Antique Silver", "Early English Antique Spoons"]
            );
        }

        /// The WooCommerce sale price of a live capture (georgianantiques.net) came out as
        /// `Original price was: £3,450.£2,950Current price is: £2,950.`.
        #[test]
        fn should_extract_the_woocommerce_sale_price_without_screen_reader_text() {
            let doc = html(
                "<div class=\"summary\"><p class=\"price\">\n  <del aria-hidden=\"true\"><span class=\"woocommerce-Price-amount amount\"><bdi><span class=\"woocommerce-Price-currencySymbol\">&pound;</span>3,450</bdi></span></del> <span class=\"screen-reader-text\">Original price was: &pound;3,450.</span><ins aria-hidden=\"true\"><span class=\"woocommerce-Price-amount amount\"><bdi><span class=\"woocommerce-Price-currencySymbol\">&pound;</span>2,950</bdi></span></ins><span class=\"screen-reader-text\">Current price is: &pound;2,950.</span>\n</p></div>",
            );
            let r = rule(".summary .price", text_kind(), ExtractionCardinality::First);

            assert_eq!(r.apply(&doc).unwrap(), vec!["£2,950"]);
        }
    }

    #[test]
    fn should_return_vec_with_single_element_for_single_match() {
        let doc = html("<p>Only</p>");
        let r = rule("p", text_kind(), ExtractionCardinality::First);
        let result = r.apply(&doc).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "Only");
    }

    // ─── ProductListing image URLs: realistic multi-selector scenario ─────────

    #[test]
    fn should_collect_all_image_urls_from_multiple_selectors_for_product_images() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <div class="main-image"><img src="main.jpg"/></div>
                <div class="thumbnails">
                    <img src="thumb1.jpg"/>
                    <img src="thumb2.jpg"/>
                </div>
                <div class="zoom-gallery">
                    <a data-full="zoom1.jpg">Z1</a>
                    <a data-full="zoom2.jpg">Z2</a>
                </div>
            </body></html>
            "#,
        );

        let r = rule_with_additional(
            ".main-image img",
            vec![".thumbnails img"],
            attr_kind("src"),
            ExtractionCardinality::All,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["main.jpg", "thumb1.jpg", "thumb2.jpg"]
        );
    }

    #[test]
    fn should_collect_image_urls_from_different_attribute_kinds_requires_separate_rules() {
        // This test demonstrates that a single rule uses one ExtractionKind for
        // all selectors. Different attributes need separate rules.
        let doc = Html::parse_document(
            r#"
            <html><body>
                <div class="gallery">
                    <img src="img1.jpg"/>
                    <img src="img2.jpg"/>
                </div>
            </body></html>
            "#,
        );

        let r = rule(".gallery img", attr_kind("src"), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec!["img1.jpg", "img2.jpg"]);
    }

    // ─── rstest parameterized tests ────────────────────────────────────

    #[test]
    fn should_order_large_image_attribute_before_thumbnail_src_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <div class="gallery">
                    <img
                        src="/uploads/photo-100x100.jpg"
                        data-large_image="/uploads/photo-scaled.jpg"
                        data-large_image_width="1536"
                        data-large_image_height="2048">
                </div>
            </body></html>
            "#,
        );

        let r = rule(".gallery img", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["/uploads/photo-scaled.jpg", "/uploads/photo-100x100.jpg"]
        );
    }

    #[test]
    fn should_order_largest_srcset_candidate_before_src_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <img class="product" src="/uploads/photo-100x100.jpg"
                     srcset="/uploads/photo-150x150.jpg 150w, /uploads/photo-768x768.jpg 768w, /uploads/photo.jpg 1200w">
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["/uploads/photo.jpg", "/uploads/photo-100x100.jpg"]
        );
    }

    #[test]
    fn should_order_parent_anchor_before_thumbnail_src_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a href="/uploads/photo-original.jpg">
                    <img class="product" src="/uploads/photo-100x100.jpg">
                </a>
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["/uploads/photo-original.jpg", "/uploads/photo-100x100.jpg"]
        );
    }

    #[test]
    fn should_include_picture_source_srcset_candidates_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <picture>
                    <source media="(min-width: 800px)"
                            srcset="/uploads/photo-640x480.webp 640w, /uploads/photo-1200x900.webp 1200w">
                    <img class="product" src="/uploads/photo-100x100.jpg">
                </picture>
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["/uploads/photo-1200x900.webp", "/uploads/photo-100x100.jpg"]
        );
    }

    #[test]
    fn should_ignore_non_image_current_href_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a class="product" href="/product/foo">ProductListing detail</a>
            </body></html>
            "#,
        );

        let r = rule("a.product", image_url_kind(), ExtractionCardinality::All);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
    }

    #[test]
    fn should_ignore_non_image_parent_href_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a href="https://catalog.example.com/category/table">
                    <img class="product" src="https://cdn.example.com/images/product/abc123">
                </a>
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["https://cdn.example.com/images/product/abc123"]
        );
    }

    #[test]
    fn should_accept_extensionless_current_href_with_image_context_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a class="full-image" href="/photos/51996">Photo</a>
            </body></html>
            "#,
        );

        let r = rule("a.full-image", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(image_candidates(&result[0]), vec!["/photos/51996"]);
    }

    #[test]
    fn should_accept_extensionless_parent_href_with_image_path_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a href="/photos/51996">
                    <img class="product" src="/thumbs/51996">
                </a>
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["/photos/51996", "/thumbs/51996"]
        );
    }

    #[test]
    fn should_accept_current_href_with_image_extension_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a class="product" href="/uploads/photo.jpg">Photo</a>
            </body></html>
            "#,
        );

        let r = rule("a.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(image_candidates(&result[0]), vec!["/uploads/photo.jpg"]);
    }

    #[test]
    fn should_accept_parent_href_with_image_extension_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a href="https://catalog.example.com/uploads/photo.webp">
                    <img class="product">
                </a>
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["https://catalog.example.com/uploads/photo.webp"]
        );
    }

    #[test]
    fn should_accept_extensionless_src_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <img class="product" src="https://cdn.example.com/images/product/abc123">
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec!["https://cdn.example.com/images/product/abc123"]
        );
    }

    #[test]
    fn should_accept_extensionless_large_image_attribute_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <img class="product"
                     src="/uploads/photo-100x100.jpg"
                     data-large_image="https://cdn.example.com/images/product/fullsize">
            </body></html>
            "#,
        );

        let r = rule("img.product", image_url_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(
            image_candidates(&result[0]),
            vec![
                "https://cdn.example.com/images/product/fullsize",
                "/uploads/photo-100x100.jpg"
            ]
        );
    }

    /// A gallery whose matched elements include elements without a value, which the
    /// `images` rule skips (#2099), up to a gallery of placeholders only (#2432).
    mod image_elements_without_candidate {
        use super::*;

        const GALLERY_SELECTOR: &str = r#"div.product-image-grid a[rel^="lightbox"]"#;

        fn image_url(index: usize) -> String {
            format!("https://files.ekmcdn.com/shop/images/side-table-({index})-5702-p.jpg")
        }

        /// Harrison Antique Furniture's gallery of twenty slots: an image link per photo,
        /// followed by empty `href="#"` slots of the same lightbox, each holding a hidden
        /// image without a host.
        fn gallery_with_placeholder_anchors(photos: usize) -> Html {
            let image_links = (1..=photos).map(|index| {
                format!(
                    r#"<div><a href="{}" rel="lightbox[1] mousezoom[1]" target="_blank"></a></div>"#,
                    image_url(index)
                )
            });
            let placeholders = (photos + 1..=20).map(|index| {
                format!(
                    r##"<div><a href="#" id="_EKM_PRODUCTIMAGE_LINK_{index}_2" rel="lightbox[1] mousezoom[1]" data-thumbnailimg="" data-largeimg="" data-video="" target="_blank"/>
<img itemprop="image" src="//:0" alt="variant attributes" width="200" height="200" id="_EKM_PRODUCTIMAGE_{index}_2" border="0" style="display:none;" /></a></div>"##
                )
            });
            Html::parse_document(&format!(
                r#"<html><body><div class="product-image-grid">{}</div></body></html>"#,
                image_links.chain(placeholders).collect::<String>()
            ))
        }

        #[test]
        fn should_count_skipped_anchors_when_extracting_image_url_candidate_groups() {
            let r = rule(
                GALLERY_SELECTOR,
                image_url_kind(),
                ExtractionCardinality::First,
            );

            let images = r
                .apply_image_url_candidate_groups(&gallery_with_placeholder_anchors(8))
                .unwrap();

            assert_eq!(images.values, (1..=8).map(image_url).collect::<Vec<_>>());
            assert_eq!(images.skipped_elements, 12);
        }

        #[test]
        fn should_take_images_of_additional_selector_when_primary_matches_only_placeholders() {
            let doc = html(
                r##"<div class="thumbs"><a href="#" rel="lightbox"></a></div>
                <div class="gallery"><img src="/uploads/photo.jpg"></div>"##,
            );
            let r = rule_with_additional(
                ".thumbs a",
                vec![".gallery img"],
                image_url_kind(),
                ExtractionCardinality::All,
            );

            let images = r.apply_image_url_candidate_groups(&doc).unwrap();

            assert_eq!(images.values, vec!["/uploads/photo.jpg"]);
            assert_eq!(images.skipped_elements, 1);
        }

        #[test]
        fn should_skip_gallery_images_without_the_attribute_of_an_attribute_rule() {
            let doc = html(
                r#"<div class="gallery"><img src="a.jpg"><img alt="no-src"><img src="c.jpg"></div>"#,
            );
            let r = rule(
                "div.gallery img",
                attr_kind("src"),
                ExtractionCardinality::All,
            );

            let images = r.apply_image_url_candidate_groups(&doc).unwrap();

            assert_eq!(images.values, vec!["a.jpg", "c.jpg"]);
            assert_eq!(images.skipped_elements, 1);
        }

        /// Only the `images` rule skips: the same rule applied to another field still
        /// fails on an element without a value.
        #[test]
        fn should_fail_other_fields_on_an_element_without_a_value() {
            let r = rule(
                GALLERY_SELECTOR,
                image_url_kind(),
                ExtractionCardinality::All,
            );

            let err = r.apply(&gallery_with_placeholder_anchors(8)).unwrap_err();

            assert!(
                matches!(&err, ExtractionError::MissingAttribute { attribute, .. } if attribute == "image URL candidate"),
                "unexpected error: {err:?}"
            );
        }

        /// The slots name their lightbox and their hidden images an image in their id, so a
        /// selector of either finds an empty gallery.
        #[rstest]
        #[case::slots(GALLERY_SELECTOR)]
        #[case::hidden_images_of_the_slots("div.product-image-grid img")]
        fn should_yield_an_empty_gallery_when_every_matched_element_is_a_placeholder(
            #[case] selector: &str,
        ) {
            let r = rule(selector, image_url_kind(), ExtractionCardinality::All);

            let images = r
                .apply_image_url_candidate_groups(&gallery_with_placeholder_anchors(0))
                .unwrap();

            assert_eq!(
                images,
                RuleExtraction {
                    values: vec![],
                    skipped_elements: 20,
                }
            );
        }

        /// Elements without a value that are not all gallery placeholders mean a selector
        /// that does not fit the page, so it never passes as an empty gallery.
        #[rstest]
        #[case::navigation_link(r##"<a href="#"></a>"##, image_url_kind())]
        #[case::labelled_control(r##"<a href="#" class="zoom">Zoom</a>"##, image_url_kind())]
        #[case::wrapper_around_its_image(
            r#"<div class="gallery-item"><img src="/uploads/photo.jpg"></div>"#,
            image_url_kind()
        )]
        #[case::image_the_rule_cannot_read(
            r#"<div class="gallery-item" data-lazy="/uploads/photo.jpg"></div>"#,
            image_url_kind()
        )]
        #[case::slot_naming_its_image_in_an_unread_attribute(
            r##"<a href="#" rel="lightbox" data-largeimg="/uploads/photo.jpg"></a>"##,
            image_url_kind()
        )]
        #[case::wrapper_around_an_unread_lazy_image(
            r#"<div class="gallery-item"><span data-bg="/uploads/photo.jpg"></span></div>"#,
            image_url_kind()
        )]
        #[case::slot_naming_its_image_next_to_a_hostless_one(
            r##"<a href="#" rel="lightbox" data-largeimg="/uploads/photo.jpg"><img src="//:0"></a>"##,
            image_url_kind()
        )]
        #[case::hostless_image_outside_an_image_context(r#"<img src="//:0">"#, image_url_kind())]
        #[case::layout_element(r#"<div class="w-full"></div>"#, image_url_kind())]
        #[case::image_link_without_the_attribute(
            r#"<a rel="lightbox" href="/uploads/photo.jpg"></a>"#,
            attr_kind("data-full")
        )]
        fn should_fail_when_an_element_without_a_value_is_not_a_placeholder(
            #[case] element: &str,
            #[case] kind: ExtractionKind,
        ) {
            let doc = html(&format!(r#"<div class="thumbs">{element}</div>"#));
            let r = rule(".thumbs > *", kind, ExtractionCardinality::All);

            let err = r.apply_image_url_candidate_groups(&doc).unwrap_err();

            assert!(
                matches!(&err, ExtractionError::MissingAttribute { selector, .. } if selector == ".thumbs > *"),
                "unexpected error: {err:?}"
            );
        }

        #[test]
        fn should_fail_with_the_error_of_the_first_element_that_is_not_a_placeholder() {
            let doc = html(
                r##"<div class="thumbs"><a href="#" rel="lightbox"></a><a href="#" rel="lightbox"></a></div>
                <div class="actions"><a href="#">Add to wishlist</a></div>"##,
            );
            let r = rule_with_additional(
                ".thumbs a",
                vec![".actions a"],
                image_url_kind(),
                ExtractionCardinality::All,
            );

            let err = r.apply_image_url_candidate_groups(&doc).unwrap_err();

            assert!(
                matches!(
                    &err,
                    ExtractionError::MissingAttribute { selector, attribute }
                        if selector == ".actions a" && attribute == "image URL candidate"
                ),
                "unexpected error: {err:?}"
            );
        }

        #[test]
        fn should_fail_when_no_matched_gallery_image_has_the_attribute_of_an_attribute_rule() {
            let doc = html(r#"<div class="gallery"><img alt="no-src"></div>"#);
            let r = rule(
                "div.gallery img",
                attr_kind("src"),
                ExtractionCardinality::All,
            );

            let err = r.apply_image_url_candidate_groups(&doc).unwrap_err();

            assert!(
                matches!(
                    &err,
                    ExtractionError::MissingAttribute { selector, attribute }
                        if selector == "div.gallery img" && attribute == "src"
                ),
                "unexpected error: {err:?}"
            );
        }

        #[test]
        fn should_fail_first_cardinality_when_first_element_has_no_image_url_candidate() {
            let doc = html(
                r##"<div class="thumbs"><a href="#" rel="lightbox"></a><a href="/uploads/photo.jpg" rel="lightbox"></a></div>"##,
            );
            let r = rule(".thumbs a", image_url_kind(), ExtractionCardinality::First);

            let err = r.apply(&doc).unwrap_err();

            assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
        }
    }

    #[rstest]
    #[case::simple_tag("h1", "<h1>Title</h1>", vec!["Title"])]
    #[case::class_selector(".info", r#"<span class="info">Info text</span>"#, vec!["Info text"])]
    #[case::nested_text("div", "<div>Outer <b>Inner</b></div>", vec!["Outer Inner"])]
    fn should_extract_text_for_various_selectors_when_first_cardinality(
        #[case] selector: &str,
        #[case] raw_html: &str,
        #[case] expected: Vec<&str>,
    ) {
        let doc = html(raw_html);
        let r = rule(selector, text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), expected);
    }

    #[rstest]
    #[case::href("a", "href", r#"<a href="http://x.com">L</a>"#, vec!["http://x.com"])]
    #[case::src("img", "src", r#"<img src="photo.jpg"/>"#, vec!["photo.jpg"])]
    #[case::alt("img", "alt", r#"<img alt="description"/>"#, vec!["description"])]
    #[case::data_attr("div", "data-id", r#"<div data-id="123">X</div>"#, vec!["123"])]
    fn should_extract_attribute_for_various_attributes_when_first_cardinality(
        #[case] selector: &str,
        #[case] attr_name: &str,
        #[case] raw_html: &str,
        #[case] expected: Vec<&str>,
    ) {
        let doc = html(raw_html);
        let r = rule(selector, attr_kind(attr_name), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), expected);
    }

    #[rstest]
    #[case::bad_bracket("[[[")]
    #[case::bad_chars("!!!")]
    #[case::empty("")]
    #[case::unclosed_paren("div:nth-child(")]
    fn should_return_invalid_selector_for_various_malformed_selectors(#[case] bad_selector: &str) {
        let doc = html("<p>Test</p>");
        let r = rule(bad_selector, text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(
            matches!(err, ExtractionError::InvalidSelector { .. }),
            "expected InvalidSelector for `{bad_selector}`, got: {err:?}"
        );
    }

    #[rstest]
    #[case::primary_matches(
        "h1", vec!["h2"],
        "<h1>A</h1><h2>B</h2>",
        vec!["A"],
    )]
    #[case::primary_blank(
        "h1", vec!["h2", "h3"],
        "<h1> </h1><h2>B</h2><h3>C</h3>",
        vec!["B"],
    )]
    #[case::middle_selector_does_not_match(
        "h1", vec!["h2", "h3"],
        "<h3>C</h3>",
        vec!["C"],
    )]
    #[case::only_additional_matches(
        "h1", vec!["h2"],
        "<h2>B</h2>",
        vec!["B"],
    )]
    fn should_fall_back_through_the_selectors_in_order_when_first_cardinality(
        #[case] primary: &str,
        #[case] additional: Vec<&str>,
        #[case] raw_html: &str,
        #[case] expected: Vec<&str>,
    ) {
        let doc = html(raw_html);
        let r = rule_with_additional(
            primary,
            additional,
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), expected);
    }

    // ─── Error Display ─────────────────────────────────────────────────

    #[test]
    fn should_display_meaningful_message_for_invalid_selector_error() {
        let err = ExtractionError::InvalidSelector {
            selector: "[[[".to_owned(),
            reason: "parse error".to_owned(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("[[["));
        assert!(msg.contains("parse error"));
    }

    #[test]
    fn should_display_meaningful_message_for_no_element_matched_error() {
        let err = ExtractionError::NoElementMatched {
            selector: "h1.missing".to_owned(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("h1.missing"));
        assert!(msg.contains("no element matched"));
    }

    #[test]
    fn should_display_meaningful_message_for_missing_attribute_error() {
        let err = ExtractionError::MissingAttribute {
            selector: "img".to_owned(),
            attribute: "src".to_owned(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("img"));
        assert!(msg.contains("src"));
    }

    // ─── Full document parsing ─────────────────────────────────────────

    #[test]
    fn should_work_with_full_html_document() {
        let doc = Html::parse_document(
            r#"
            <!DOCTYPE html>
            <html>
            <head><title>Test</title></head>
            <body>
                <h1 class="main-title">ProductListing Name</h1>
                <div class="price">$99.99</div>
                <div class="images">
                    <img src="img1.jpg" alt="Front"/>
                    <img src="img2.jpg" alt="Back"/>
                </div>
            </body>
            </html>
            "#,
        );

        let title_rule = rule(".main-title", text_kind(), ExtractionCardinality::First);
        assert_eq!(title_rule.apply(&doc).unwrap(), vec!["ProductListing Name"]);

        let price_rule = rule(".price", text_kind(), ExtractionCardinality::First);
        assert_eq!(price_rule.apply(&doc).unwrap(), vec!["$99.99"]);

        let images_rule = rule(".images img", attr_kind("src"), ExtractionCardinality::All);
        assert_eq!(
            images_rule.apply(&doc).unwrap(),
            vec!["img1.jpg", "img2.jpg"]
        );
    }

    #[test]
    fn should_take_the_old_layout_price_only_when_a_page_still_renders_both_layouts() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <span class="product-price">€19.90</span>
                <span class="product-price-v2">€29.90</span>
            </body></html>
            "#,
        );

        let r = rule_with_additional(
            ".product-price",
            vec![".product-price-v2"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["€19.90"]);
    }

    #[test]
    fn should_collect_images_from_multiple_gallery_sections() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <div class="primary-gallery">
                    <img src="primary1.jpg"/>
                    <img src="primary2.jpg"/>
                </div>
                <div class="secondary-gallery">
                    <img src="secondary1.jpg"/>
                </div>
            </body></html>
            "#,
        );

        let r = rule_with_additional(
            ".primary-gallery img",
            vec![".secondary-gallery img"],
            attr_kind("src"),
            ExtractionCardinality::All,
        );
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["primary1.jpg", "primary2.jpg", "secondary1.jpg"]
        );
    }

    #[test]
    fn should_collect_the_first_image_of_the_first_gallery_only_with_first_cardinality() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <div class="primary-gallery">
                    <img src="primary1.jpg"/>
                    <img src="primary2.jpg"/>
                </div>
                <div class="secondary-gallery">
                    <img src="secondary1.jpg"/>
                    <img src="secondary2.jpg"/>
                </div>
            </body></html>
            "#,
        );

        let r = rule_with_additional(
            ".primary-gallery img",
            vec![".secondary-gallery img"],
            attr_kind("src"),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["primary1.jpg"]);
    }

    // ─── attribute-or-text and JSON-LD sources ─────────────────────────

    fn attribute_or_text(name: &str) -> ExtractionKind {
        ExtractionKind::AttributeOrText { name: name.into() }
    }

    fn json_ld(property: JsonLdOfferProperty) -> ExtractionKind {
        ExtractionKind::JsonLdOffer { property }
    }

    #[rstest]
    #[case::attribute_wins(
        r#"<input class="cta" type="submit" value="  Add   to enquiry ">"#,
        "Add to enquiry"
    )]
    #[case::text_when_attribute_missing(r#"<button class="cta">Reserve</button>"#, "Reserve")]
    #[case::text_when_attribute_blank(
        r#"<button class="cta" value=" ">Buy now</button>"#,
        "Buy now"
    )]
    fn should_read_an_attribute_or_else_the_text(#[case] markup: &str, #[case] expected: &str) {
        let rule = rule(
            ".cta",
            attribute_or_text("value"),
            ExtractionCardinality::First,
        );
        assert_eq!(rule.apply(&html(markup)).unwrap(), vec![expected]);
    }

    #[test]
    fn should_fail_an_attribute_or_text_rule_whose_elements_show_nothing_as_empty() {
        let rule = rule(
            ".cta",
            attribute_or_text("value"),
            ExtractionCardinality::First,
        );
        let error = rule
            .apply(&html(r#"<button class="cta"></button>"#))
            .unwrap_err();
        assert!(matches!(error, ExtractionError::EmptyValue { .. }));
    }

    #[test]
    fn should_mix_attribute_and_text_elements_with_cardinality_all() {
        let rule = rule(".cta", attribute_or_text("alt"), ExtractionCardinality::All);
        let page =
            html(r#"<img class="cta" alt="ITEM HAS BEEN SOLD"><span class="cta">Enquire</span>"#);
        assert_eq!(
            rule.apply(&page).unwrap(),
            vec!["ITEM HAS BEEN SOLD", "Enquire"]
        );
    }

    const LD_SCRIPT: &str = r#"<script type="application/ld+json">{"@type":"Product","offers":{"price":"2800","availability":"https://schema.org/InStock"}}</script>"#;

    #[rstest]
    #[case(JsonLdOfferProperty::Availability, "https://schema.org/InStock")]
    #[case(JsonLdOfferProperty::Price, "2800")]
    fn should_read_json_ld_offers(#[case] property: JsonLdOfferProperty, #[case] expected: &str) {
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            json_ld(property),
            ExtractionCardinality::First,
        );
        assert_eq!(rule.apply(&html(LD_SCRIPT)).unwrap(), vec![expected]);
    }

    #[test]
    fn should_take_the_first_script_that_holds_an_offer() {
        let page = html(&format!(
            r#"<script type="application/ld+json">{{"@type":"BreadcrumbList"}}</script>{LD_SCRIPT}"#
        ));
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            json_ld(JsonLdOfferProperty::Price),
            ExtractionCardinality::First,
        );
        assert_eq!(rule.apply(&page).unwrap(), vec!["2800"]);
    }

    #[test]
    fn should_fail_a_json_ld_rule_without_an_offer_value_as_empty() {
        let page = html(r#"<script type="application/ld+json">{"@type":"Product"}</script>"#);
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            json_ld(JsonLdOfferProperty::Availability),
            ExtractionCardinality::First,
        );
        assert!(matches!(
            rule.apply(&page).unwrap_err(),
            ExtractionError::EmptyValue { .. }
        ));
    }

    const LD_PRODUCT_SCRIPT: &str = r#"<script type="application/ld+json">{"@type":"Product","productID":"9441317814427","sku":"R-1"}</script>"#;

    #[rstest]
    #[case(JsonLdProductProperty::ProductId, "9441317814427")]
    #[case(JsonLdProductProperty::Sku, "R-1")]
    fn should_read_json_ld_product_identifiers(
        #[case] property: JsonLdProductProperty,
        #[case] expected: &str,
    ) {
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            ExtractionKind::JsonLdProduct { property },
            ExtractionCardinality::First,
        );
        assert_eq!(
            rule.apply(&html(LD_PRODUCT_SCRIPT)).unwrap(),
            vec![expected]
        );
    }

    #[test]
    fn should_skip_a_json_ld_script_without_the_product_value_for_the_next_one() {
        let page = html(&format!(
            r#"<script type="application/ld+json">{{"@type":"BreadcrumbList"}}</script>{LD_PRODUCT_SCRIPT}"#
        ));
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            ExtractionKind::JsonLdProduct {
                property: JsonLdProductProperty::Sku,
            },
            ExtractionCardinality::First,
        );
        assert_eq!(rule.apply(&page).unwrap(), vec!["R-1"]);
    }

    #[test]
    fn should_fail_a_json_ld_product_rule_on_unparseable_json_as_empty() {
        let page = html(r#"<script type="application/ld+json">{"@type":"Product",</script>"#);
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            ExtractionKind::JsonLdProduct {
                property: JsonLdProductProperty::ProductId,
            },
            ExtractionCardinality::First,
        );
        assert!(matches!(
            rule.apply(&page).unwrap_err(),
            ExtractionError::EmptyValue { .. }
        ));
    }

    #[rstest]
    #[case::attribute_or_text(
        attribute_or_text("value"),
        r#"<button class="v" value=" "></button>"#
    )]
    #[case::json_ld(
        json_ld(JsonLdOfferProperty::Price),
        r#"<script class="v" type="application/ld+json">{"offers":{"price":"0.00"}}</script>"#
    )]
    fn should_report_an_all_rule_of_a_text_like_kind_without_values_as_empty(
        #[case] kind: ExtractionKind,
        #[case] markup: &str,
    ) {
        // Like a text rule, and unlike an attribute rule, which yields its first blank.
        let rule = rule(".v", kind, ExtractionCardinality::All);
        assert!(matches!(
            rule.apply(&html(markup)).unwrap_err(),
            ExtractionError::EmptyValue { .. }
        ));
    }

    #[test]
    fn should_count_only_elements_with_a_value_as_matches() {
        let page = html(&format!(
            r#"<script type="application/ld+json">{{}}</script>{LD_SCRIPT}"#
        ));
        let rule = rule(
            r#"script[type="application/ld+json"]"#,
            json_ld(JsonLdOfferProperty::Price),
            ExtractionCardinality::First,
        );
        let selector = Selector::parse(&rule.selector).unwrap();
        assert_eq!(rule.match_count(&page, &selector), 1);
        assert_eq!(rule.empty_element_count(&page, &selector), 1);
    }

    #[test]
    fn should_read_back_stored_rules_unchanged_and_the_new_kinds() {
        let stored: ExtractionRule =
            serde_json::from_str(r#"{"selector":"h1","type":"attribute","name":"content"}"#)
                .unwrap();
        assert_eq!(
            stored.extract,
            ExtractionKind::Attribute {
                name: "content".into()
            }
        );

        for (json, kind) in [
            (
                r#"{"selector":"s","type":"attribute_or_text","name":"value"}"#,
                attribute_or_text("value"),
            ),
            (
                r#"{"selector":"s","type":"json_ld_offer","property":"price"}"#,
                json_ld(JsonLdOfferProperty::Price),
            ),
            (
                r#"{"selector":"s","type":"json_ld_product","property":"product_id"}"#,
                ExtractionKind::JsonLdProduct {
                    property: JsonLdProductProperty::ProductId,
                },
            ),
            (
                r#"{"selector":"s","type":"json_ld_product","property":"sku"}"#,
                ExtractionKind::JsonLdProduct {
                    property: JsonLdProductProperty::Sku,
                },
            ),
        ] {
            let rule: ExtractionRule = serde_json::from_str(json).unwrap();
            assert_eq!(rule.extract, kind);
            let round_trip: ExtractionRule =
                serde_json::from_value(serde_json::to_value(&rule).unwrap()).unwrap();
            assert_eq!(round_trip, rule);
        }
    }
}
