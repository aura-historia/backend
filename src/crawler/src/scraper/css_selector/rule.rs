use schemars::JsonSchema;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

    #[schemars(description = "Additional CSS-Selectors for the values to extract")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_selectors: Vec<CssSelector>,

    #[schemars(description = "Kind value to extract with CSS-Selector")]
    #[serde(flatten)]
    pub extract: ExtractionKind,

    #[schemars(description = "How many values to extract")]
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

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Clone, Debug, thiserror::Error)]
pub enum ExtractionError {
    #[error("invalid CSS selector `{selector}`: {reason}")]
    InvalidSelector { selector: String, reason: String },

    #[error("no element matched selector `{selector}` (or any additional selectors)")]
    NoElementMatched { selector: String },

    #[error("element matched by `{selector}` does not have attribute `{attribute}`")]
    MissingAttribute { selector: String, attribute: String },
}

impl ExtractionRule {
    /// Apply this extraction rule to the given parsed HTML document.
    ///
    /// Every configured selector (primary + additional) is evaluated in order.
    /// Results from each selector are aggregated into a single `Vec<String>`.
    ///
    /// - **`First` cardinality**: takes the first matched element per selector.
    ///   For text extraction, empty/whitespace-only matches are skipped until a
    ///   non-empty value is found.
    /// - **`All` cardinality**: takes all matched elements per selector.
    ///
    /// Returns `Err(NoElementMatched)` only when *no* selector produced any
    /// usable value.
    pub fn apply(&self, html: &Html) -> Result<Vec<String>, ExtractionError> {
        let all_selectors = std::iter::once(&self.selector).chain(self.additional_selectors.iter());

        let mut results: Vec<String> = Vec::new();

        for css_selector in all_selectors {
            let parsed = parse_selector(css_selector)?;
            let elements = html.select(&parsed);

            match self.cardinality {
                ExtractionCardinality::First => match &self.extract {
                    ExtractionKind::Text => {
                        if let Some(value) =
                            extract_first_non_empty_text(elements, css_selector.as_ref())?
                        {
                            results.push(value);
                        }
                    }
                    ExtractionKind::Attribute { .. } | ExtractionKind::ImageUrl => {
                        let mut elements = elements.peekable();
                        if let Some(element) = elements.next() {
                            results.push(extract_from_element(
                                &element,
                                &self.extract,
                                css_selector.as_ref(),
                            )?);
                        }
                    }
                },
                ExtractionCardinality::All => {
                    for el in elements {
                        results.push(extract_from_element(
                            &el,
                            &self.extract,
                            css_selector.as_ref(),
                        )?);
                    }
                }
            }
        }

        if results.is_empty() {
            return Err(ExtractionError::NoElementMatched {
                selector: self.selector.to_string(),
            });
        }

        Ok(results)
    }

    pub(crate) fn apply_image_url_candidate_groups(
        &self,
        html: &Html,
    ) -> Result<Vec<String>, ExtractionError> {
        let mut all_images_rule = self.clone();
        all_images_rule.cardinality = ExtractionCardinality::All;
        all_images_rule.apply(html)
    }
}

fn parse_selector(css_selector: &CssSelector) -> Result<Selector, ExtractionError> {
    Selector::parse(css_selector.as_ref()).map_err(|err| ExtractionError::InvalidSelector {
        selector: css_selector.to_string(),
        reason: format!("{err:?}"),
    })
}

fn extract_first_non_empty_text(
    elements: scraper::html::Select<'_, '_>,
    selector_str: &str,
) -> Result<Option<String>, ExtractionError> {
    for element in elements {
        let value = extract_from_element(&element, &ExtractionKind::Text, selector_str)?;
        if !value.is_empty() {
            return Ok(Some(value));
        }
    }

    Ok(None)
}

fn extract_from_element(
    element: &scraper::ElementRef<'_>,
    kind: &ExtractionKind,
    selector_str: &str,
) -> Result<String, ExtractionError> {
    match kind {
        ExtractionKind::Text => {
            let text: String = element.text().collect::<Vec<_>>().join("");
            Ok(text.trim().to_owned())
        }
        ExtractionKind::Attribute { name } => {
            let attr_value = element.value().attr(name.as_ref()).ok_or_else(|| {
                ExtractionError::MissingAttribute {
                    selector: selector_str.to_owned(),
                    attribute: name.to_string(),
                }
            })?;
            Ok(attr_value.to_owned())
        }
        ExtractionKind::ImageUrl => extract_image_url_from_element(element, selector_str),
    }
}

fn extract_image_url_from_element(
    element: &scraper::ElementRef<'_>,
    selector_str: &str,
) -> Result<String, ExtractionError> {
    let candidates = image_url_candidates_for_element(element);
    if candidates.is_empty() {
        return Err(ExtractionError::MissingAttribute {
            selector: selector_str.to_owned(),
            attribute: "image URL candidate".to_owned(),
        });
    }

    Ok(candidates.join(IMAGE_CANDIDATE_SEPARATOR.encode_utf8(&mut [0; 4])))
}

fn image_url_candidates_for_element(element: &scraper::ElementRef<'_>) -> Vec<String> {
    let mut candidates = OrderedImageCandidates::default();

    for attr in [
        "data-large_image",
        "data-full",
        "data-original",
        "data-zoom-image",
    ] {
        push_attr_candidate(
            &mut candidates,
            element,
            attr,
            ImageCandidateSource::ImageSpecific,
        );
    }
    for attr in ["data-src", "data-lazy-src", "content"] {
        push_attr_candidate(
            &mut candidates,
            element,
            attr,
            ImageCandidateSource::ImageSpecific,
        );
    }
    push_attr_candidate(
        &mut candidates,
        element,
        "href",
        image_link_source_for_element(element, element.value().attr("href")),
    );

    if let Some((href, source)) = nearest_anchor_href(element) {
        candidates.push(href, source);
    }

    for srcset in picture_source_srcsets(element) {
        if let Some((url, _)) = largest_srcset_candidate(Some(srcset)) {
            candidates.push(url, ImageCandidateSource::ImageSpecific);
        }
    }
    if let Some((url, _)) = largest_srcset_candidate(element.value().attr("srcset")) {
        candidates.push(url, ImageCandidateSource::ImageSpecific);
    }

    push_attr_candidate(
        &mut candidates,
        element,
        "src",
        ImageCandidateSource::ImageSpecific,
    );

    candidates.into_vec()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageCandidateSource {
    ImageSpecific,
    ImageLikeLink,
    GenericLink,
}

#[derive(Default)]
struct OrderedImageCandidates {
    seen: HashSet<String>,
    values: Vec<String>,
}

impl OrderedImageCandidates {
    fn push(&mut self, value: String, source: ImageCandidateSource) {
        let trimmed = value.trim();
        if trimmed.is_empty() || !is_probably_image_url(trimmed, source) {
            return;
        }

        let normalized = trimmed.to_owned();
        if self.seen.insert(normalized.clone()) {
            self.values.push(normalized);
        }
    }

    fn into_vec(self) -> Vec<String> {
        self.values
    }
}

fn push_attr_candidate(
    candidates: &mut OrderedImageCandidates,
    element: &scraper::ElementRef<'_>,
    attr: &str,
    source: ImageCandidateSource,
) {
    if let Some(value) = element
        .value()
        .attr(attr)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        candidates.push(value.to_owned(), source);
    }
}

fn nearest_anchor_href(
    element: &scraper::ElementRef<'_>,
) -> Option<(String, ImageCandidateSource)> {
    for ancestor in element.ancestors() {
        let Some(ancestor) = scraper::ElementRef::wrap(ancestor) else {
            continue;
        };
        if ancestor.value().name() == "a" {
            let href = ancestor.value().attr("href")?;
            return Some((
                href.to_owned(),
                image_link_source_for_element(&ancestor, Some(href)),
            ));
        }
    }
    None
}

fn image_link_source_for_element(
    element: &scraper::ElementRef<'_>,
    href: Option<&str>,
) -> ImageCandidateSource {
    if href.is_some_and(path_has_image_context) || element_attrs_have_image_context(element) {
        ImageCandidateSource::ImageLikeLink
    } else {
        ImageCandidateSource::GenericLink
    }
}

fn element_attrs_have_image_context(element: &scraper::ElementRef<'_>) -> bool {
    const IMAGE_CONTEXT_TERMS: &[&str] = &[
        "image",
        "images",
        "img",
        "photo",
        "photos",
        "gallery",
        "thumbnail",
        "thumb",
        "zoom",
        "lightbox",
        "fancybox",
        "prettyphoto",
        "full",
    ];

    let attr_text = [
        element.value().attr("class"),
        element.value().attr("id"),
        element.value().attr("rel"),
        element.value().attr("data-gallery"),
        element.value().attr("data-lightbox"),
        element.value().attr("data-fancybox"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();

    IMAGE_CONTEXT_TERMS
        .iter()
        .any(|term| attr_text.contains(term))
}

fn path_has_image_context(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    [
        "/photo", "/photos", "/image", "/images", "/media", "/uploads", "/cdn/", "cdn.",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn picture_source_srcsets<'a>(element: &scraper::ElementRef<'a>) -> Vec<&'a str> {
    let Some(parent_node) = element.parent() else {
        return Vec::new();
    };
    let Some(parent) = scraper::ElementRef::wrap(parent_node) else {
        return Vec::new();
    };
    if parent.value().name() != "picture" {
        return Vec::new();
    }

    parent
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .filter(|child| child.value().name() == "source")
        .filter_map(|child| child.value().attr("srcset"))
        .collect()
}

fn largest_srcset_candidate(srcset: Option<&str>) -> Option<(String, usize)> {
    srcset?
        .split(',')
        .filter_map(|candidate| {
            let mut parts = candidate.split_whitespace();
            let url = parts.next()?.trim();
            let width = parts
                .find_map(|part| part.strip_suffix('w'))
                .and_then(|raw| raw.parse::<usize>().ok())
                .unwrap_or_default();
            if url.is_empty() {
                None
            } else {
                Some((url.to_owned(), width))
            }
        })
        .max_by_key(|(_, width)| *width)
}

fn is_probably_image_url(url: &str, source: ImageCandidateSource) -> bool {
    match source {
        ImageCandidateSource::ImageSpecific => is_url_like(url) || has_known_image_extension(url),
        ImageCandidateSource::ImageLikeLink => is_url_like(url) || has_known_image_extension(url),
        ImageCandidateSource::GenericLink => has_known_image_extension(url),
    }
}

fn is_url_like(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with('/')
}

fn has_known_image_extension(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".png")
        || lower.contains(".webp")
        || lower.contains(".gif")
        || lower.contains(".avif")
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
    fn should_return_no_element_matched_when_only_empty_text_is_found_for_first_cardinality() {
        let doc = html("<div><br/></div>");
        let r = rule("br", text_kind(), ExtractionCardinality::First);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
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

    // ─── Additional selectors: aggregation ─────────────────────────────

    #[test]
    fn should_aggregate_results_from_primary_and_additional_selectors_in_order() {
        let doc = html("<h1>Primary</h1><h2>Additional</h2>");
        let r = rule_with_additional("h1", vec!["h2"], text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Primary", "Additional"]);
    }

    #[test]
    fn should_aggregate_results_from_all_three_selectors_in_order() {
        let doc = html("<h1>First</h1><h2>Second</h2><h3>Third</h3>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3"],
            text_kind(),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["First", "Second", "Third"]);
    }

    #[test]
    fn should_skip_non_matching_additional_selector_without_error() {
        let doc = html("<h1>Primary</h1><h3>Third</h3>");
        let r = rule_with_additional(
            "h1",
            vec!["h2", "h3"],
            text_kind(),
            ExtractionCardinality::First,
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
    fn should_aggregate_first_element_per_selector_for_first_cardinality() {
        let doc = html(
            r#"<div class="gallery"><img src="a.png"/><img src="b.png"/></div><div class="extra"><img src="c.png"/><img src="d.png"/></div>"#,
        );
        let r = rule_with_additional(
            ".gallery img",
            vec![".extra img"],
            attr_kind("src"),
            ExtractionCardinality::First,
        );
        assert_eq!(r.apply(&doc).unwrap(), vec!["a.png", "c.png"]);
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
        let r = rule_with_additional("h3", vec!["h2"], text_kind(), ExtractionCardinality::First);
        assert_eq!(r.apply(&doc).unwrap(), vec!["Three", "Two"]);
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
    fn should_return_no_element_matched_when_all_text_matches_are_empty() {
        let doc = html(r#"<div class="price"></div><div class="fallback">   </div>"#);
        let r = rule_with_additional(
            ".price",
            vec![".fallback"],
            text_kind(),
            ExtractionCardinality::First,
        );
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::NoElementMatched { .. }));
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
        let doc = html("<h1>Valid</h1>");
        let r = rule_with_additional(
            "h1",
            vec!["[[[bad"],
            text_kind(),
            ExtractionCardinality::First,
        );
        // Primary matches, but the additional selector is still parsed → error
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::InvalidSelector { .. }));
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
        let doc = html(r#"<a href="ok.html">Link</a><span>No href</span>"#);
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
    fn should_reject_extensionless_product_href_for_image_url_extraction() {
        let doc = Html::parse_document(
            r#"
            <html><body>
                <a class="product" href="/products/foo">ProductListing detail</a>
            </body></html>
            "#,
        );

        let r = rule("a.product", image_url_kind(), ExtractionCardinality::All);
        let err = r.apply(&doc).unwrap_err();
        assert!(matches!(err, ExtractionError::MissingAttribute { .. }));
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
    #[case::two_selectors_both_match(
        "h1", vec!["h2"],
        "<h1>A</h1><h2>B</h2>",
        vec!["A", "B"],
    )]
    #[case::three_selectors_all_match(
        "h1", vec!["h2", "h3"],
        "<h1>A</h1><h2>B</h2><h3>C</h3>",
        vec!["A", "B", "C"],
    )]
    #[case::middle_selector_does_not_match(
        "h1", vec!["h2", "h3"],
        "<h1>A</h1><h3>C</h3>",
        vec!["A", "C"],
    )]
    #[case::only_additional_matches(
        "h1", vec!["h2"],
        "<h2>B</h2>",
        vec!["B"],
    )]
    fn should_aggregate_results_for_various_selector_combinations_when_first_cardinality(
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
    fn should_aggregate_results_in_realistic_product_page_with_additional_selectors() {
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
        assert_eq!(r.apply(&doc).unwrap(), vec!["€19.90", "€29.90"]);
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
    fn should_collect_first_image_per_gallery_section_with_first_cardinality() {
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
        assert_eq!(
            r.apply(&doc).unwrap(),
            vec!["primary1.jpg", "secondary1.jpg"]
        );
    }
}
