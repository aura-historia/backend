use image_url::image_url_candidate_group;
use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use visible_text::{TextBoundaries, visible_text};

pub(crate) use image_url::has_known_image_extension;

mod image_url;
mod visible_text;

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
        description = "Additional CSS-Selectors, evaluated after the primary selector in order. With cardinality `first` they are fallbacks: the first selector that yields a value wins and the later ones are not evaluated. With cardinality `all` they aggregate: the matches of every selector are collected in selector order, each element at most once; an element nested inside, or containing, an element the rule already extracted is skipped, so a container and its paragraphs are never captured together."
    )]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_selectors: Vec<CssSelector>,

    #[schemars(description = "Kind value to extract with CSS-Selector")]
    #[serde(flatten)]
    pub extract: ExtractionKind,

    #[schemars(
        description = "How many values to extract: `first` (default) takes the first value of the first selector that yields one, `all` takes every matched element of every selector."
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
    Attribute { name: HtmlAttributeName },
    ImageUrl,
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
    /// Skips and counts the element (see [`RuleExtraction::skipped_elements`]).
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
        if let ExtractionKind::Attribute { name } = &self.extract
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
    ///   selector that yields a value wins and the later ones are not evaluated. For
    ///   text extraction, empty/whitespace-only matches are skipped until a non-empty
    ///   value is found.
    /// - **`All` cardinality**: the additional selectors aggregate. Every matched element
    ///   of every selector is taken, in selector order, each element at most once: an
    ///   element nested inside, or containing, an element the rule already extracted is
    ///   skipped, so a description container and its own paragraphs are never captured
    ///   together.
    ///
    /// Text extraction separates lines at `<br>` and block boundaries, and table cells
    /// with a space, wherever markup whitespace does not already separate them.
    ///
    /// When no selector yields a value, a text rule with cardinality `first` that matched
    /// only empty elements returns `Err(EmptyValue)`; a rule that matched no element
    /// returns `Err(NoElementMatched)`.
    pub fn apply(&self, html: &Html) -> Result<Vec<String>, ExtractionError> {
        self.extract(html, TextBoundaries::Separate, ElementWithoutValue::Fails)
            .map(|extraction| extraction.values)
    }

    /// Applies this rule like [`Self::apply`], except that text joins its nodes as written
    /// instead of separating lines at `<br>` and block boundaries. A captured listing
    /// identity must not change with how text is laid out: a changed identity captures the
    /// product again as a new listing.
    pub(crate) fn apply_as_identity(&self, html: &Html) -> Result<Vec<String>, ExtractionError> {
        self.extract(html, TextBoundaries::Ignore, ElementWithoutValue::Fails)
            .map(|extraction| extraction.values)
    }

    /// Applies this rule as the `images` rule: with cardinality `all`, whatever its
    /// configured cardinality, so every gallery element yields its ordered image URL
    /// candidates as one group. A matched element without a value (no image URL
    /// candidate, or no attribute of an attribute rule) is skipped and counted; the rule
    /// fails with the first skipped element's error only when it skips every element it
    /// matched.
    pub(crate) fn apply_image_url_candidate_groups(
        &self,
        html: &Html,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut all_images_rule = self.clone();
        all_images_rule.cardinality = ExtractionCardinality::All;
        all_images_rule.extract(html, TextBoundaries::Separate, ElementWithoutValue::Skipped)
    }

    fn extract(
        &self,
        html: &Html,
        boundaries: TextBoundaries,
        without_value: ElementWithoutValue,
    ) -> Result<RuleExtraction, ExtractionError> {
        match self.cardinality {
            ExtractionCardinality::First => self.extract_first(html, boundaries),
            ExtractionCardinality::All => self.extract_all(html, boundaries, without_value),
        }
    }

    /// Cardinality `first`: the first value of the first selector that yields one. The
    /// later selectors are fallbacks for pages where the earlier ones miss.
    fn extract_first(
        &self,
        html: &Html,
        boundaries: TextBoundaries,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut first_selector_with_empty_elements = None;
        for css_selector in self.selectors() {
            let parsed = parse_selector(css_selector)?;
            match self.first_value(html.select(&parsed), css_selector, boundaries)? {
                SelectorValue::Value(value) => {
                    return Ok(RuleExtraction {
                        values: vec![value],
                        skipped_elements: 0,
                    });
                }
                SelectorValue::OnlyEmptyElements => {
                    first_selector_with_empty_elements.get_or_insert(css_selector);
                }
                SelectorValue::NoElement => {}
            }
        }

        Err(match first_selector_with_empty_elements {
            Some(selector) => ExtractionError::EmptyValue {
                selector: selector.to_string(),
            },
            None => self.no_element_matched(),
        })
    }

    /// Cardinality `all`: every element of every selector, in selector order, each
    /// element at most once. An element nested inside, or containing, an element the
    /// rule already extracted holds the same content and is skipped.
    fn extract_all(
        &self,
        html: &Html,
        boundaries: TextBoundaries,
        without_value: ElementWithoutValue,
    ) -> Result<RuleExtraction, ExtractionError> {
        let mut extraction = RuleExtraction::default();
        let mut extracted = ExtractedElements::new();
        let mut first_skipped_error = None;

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
                match extract_from_element(&element, &self.extract, css_selector, boundaries) {
                    Ok(value) => {
                        extracted.insert(element.id());
                        extraction.values.push(value);
                    }
                    Err(error) if without_value == ElementWithoutValue::Skipped => {
                        extraction.skipped_elements += 1;
                        first_skipped_error.get_or_insert(error);
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        if extraction.values.is_empty() {
            return Err(first_skipped_error.unwrap_or_else(|| self.no_element_matched()));
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
    /// misses. Attribute and image URL rules count every element.
    pub(crate) fn match_count(&self, html: &Html, selector: &Selector) -> usize {
        let elements = html.select(selector);
        match self.extract {
            ExtractionKind::Text => elements.filter(has_visible_text).count(),
            ExtractionKind::Attribute { .. } | ExtractionKind::ImageUrl => elements.count(),
        }
    }

    /// How many of the elements `selector` matches hold no visible text, for a text rule:
    /// the elements [`Self::match_count`] leaves out. `0` for attribute and image URL
    /// rules, whose match count includes every element.
    pub(crate) fn empty_element_count(&self, html: &Html, selector: &Selector) -> usize {
        match self.extract {
            ExtractionKind::Text => html
                .select(selector)
                .filter(|element| !has_visible_text(element))
                .count(),
            ExtractionKind::Attribute { .. } | ExtractionKind::ImageUrl => 0,
        }
    }

    /// The value of the first element `css_selector` matched; a text rule takes the first
    /// non-empty text instead.
    fn first_value(
        &self,
        mut elements: scraper::html::Select<'_, '_>,
        css_selector: &CssSelector,
        boundaries: TextBoundaries,
    ) -> Result<SelectorValue, ExtractionError> {
        match &self.extract {
            ExtractionKind::Text => first_non_empty_text(elements, css_selector, boundaries),
            kind @ (ExtractionKind::Attribute { .. } | ExtractionKind::ImageUrl) => {
                match elements.next() {
                    Some(element) => extract_from_element(&element, kind, css_selector, boundaries)
                        .map(SelectorValue::Value),
                    None => Ok(SelectorValue::NoElement),
                }
            }
        }
    }
}

/// What one selector of a rule with cardinality `first` found on the page.
enum SelectorValue {
    Value(String),
    /// A text rule matched elements, but none has visible text.
    OnlyEmptyElements,
    NoElement,
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

fn first_non_empty_text(
    elements: scraper::html::Select<'_, '_>,
    selector_str: &str,
    boundaries: TextBoundaries,
) -> Result<SelectorValue, ExtractionError> {
    let mut found = SelectorValue::NoElement;
    for element in elements {
        let value =
            extract_from_element(&element, &ExtractionKind::Text, selector_str, boundaries)?;
        if !value.is_empty() {
            return Ok(SelectorValue::Value(value));
        }
        found = SelectorValue::OnlyEmptyElements;
    }

    Ok(found)
}

fn extract_from_element(
    element: &scraper::ElementRef<'_>,
    kind: &ExtractionKind,
    selector_str: &str,
    boundaries: TextBoundaries,
) -> Result<String, ExtractionError> {
    match kind {
        ExtractionKind::Text => Ok(visible_text(element, boundaries).trim().to_owned()),
        ExtractionKind::Attribute { name } => {
            let attr_value = element.value().attr(name.as_ref()).ok_or_else(|| {
                ExtractionError::MissingAttribute {
                    selector: selector_str.to_owned(),
                    attribute: name.to_string(),
                }
            })?;
            Ok(attr_value.to_owned())
        }
        ExtractionKind::ImageUrl => {
            image_url_candidate_group(element).ok_or_else(|| ExtractionError::MissingAttribute {
                selector: selector_str.to_owned(),
                attribute: "image URL candidate".to_owned(),
            })
        }
    }
}

/// Whether a text rule extracts a value from `element`. One without visible text yields
/// a blank value, which `first` cardinality skips. Line separators only ever stand
/// between visible text, so they never make a blank element count as text.
fn has_visible_text(element: &scraper::ElementRef<'_>) -> bool {
    !visible_text(element, TextBoundaries::Separate)
        .trim()
        .is_empty()
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
    fn should_return_empty_text_when_selected_element_is_a_non_text_element() {
        let doc = html("<div><script>window.a = 1;</script><p>Visible</p></div>");
        let r = rule("script", text_kind(), ExtractionCardinality::All);
        assert_eq!(r.apply(&doc).unwrap(), vec![""]);
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
            vec!["Provenance: Château", "Oak,  two drawers"]
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
    fn should_handle_multiple_empty_text_nodes_for_all_cardinality() {
        let doc = html("<ul><li>  </li><li></li><li>Real</li></ul>");
        let r = rule("li", text_kind(), ExtractionCardinality::All);
        let result = r.apply(&doc).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "");
        assert_eq!(result[1], "");
        assert_eq!(result[2], "Real");
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
    /// `images` rule skips (#2099).
    mod image_elements_without_candidate {
        use super::*;

        const GALLERY_SELECTOR: &str = r#"div.product-image-grid a[rel^="lightbox"]"#;

        fn image_url(index: usize) -> String {
            format!("https://files.ekmcdn.com/shop/images/side-table-({index})-5702-p.jpg")
        }

        /// Harrison Antique Furniture's gallery of a product with eight photos: eight
        /// image links followed by twelve empty `href="#"` slots of the same lightbox.
        fn gallery_with_placeholder_anchors() -> Html {
            let image_links = (1..=8).map(|index| {
                format!(
                    r#"<div><a href="{}" rel="lightbox[1] mousezoom[1]" target="_blank"></a></div>"#,
                    image_url(index)
                )
            });
            let placeholders = (9..=20).map(|index| {
                format!(
                    r##"<div><a href="#" id="_EKM_PRODUCTIMAGE_LINK_{index}_2" rel="lightbox[1] mousezoom[1]" data-largeimg="" target="_blank"></a></div>"##
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
                .apply_image_url_candidate_groups(&gallery_with_placeholder_anchors())
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

            let err = r.apply(&gallery_with_placeholder_anchors()).unwrap_err();

            assert!(
                matches!(&err, ExtractionError::MissingAttribute { attribute, .. } if attribute == "image URL candidate"),
                "unexpected error: {err:?}"
            );
        }

        #[test]
        fn should_fail_when_no_matched_element_has_an_image_url_candidate() {
            let doc = html(
                r##"<div class="thumbs"><a href="#" rel="lightbox"></a><a href="#" rel="lightbox"></a></div>"##,
            );
            let r = rule_with_additional(
                ".missing",
                vec![".thumbs a"],
                image_url_kind(),
                ExtractionCardinality::All,
            );

            let err = r.apply_image_url_candidate_groups(&doc).unwrap_err();

            assert!(
                matches!(
                    &err,
                    ExtractionError::MissingAttribute { selector, attribute }
                        if selector == ".thumbs a" && attribute == "image URL candidate"
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
}
