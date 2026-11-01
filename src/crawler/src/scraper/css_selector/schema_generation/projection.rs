use crate::scraper::css_selector::rule::{has_known_image_extension, srcset_descriptor_width};
use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use serde::Serialize;
use std::collections::BTreeMap;

const DSL_TEXT_LIMIT: usize = 180;
const DSL_ATTR_LIMIT: usize = 250;
const DSL_IMAGE_ATTR_LIMIT: usize = 1_000;
const DSL_NODE_LIMIT: usize = 2_000;
/// The most characters of YAML one projection takes: the first elements of the page in document
/// order that fit, with the number of those left out. Navigation, header and footer are gone by
/// then, so a page's product content comes early: on every parser fixture within the first
/// 92,000 characters, the deepest behind a shop's 55,000-character currency selector. Image-heavy
/// pages reached 120,000 to 220,000 characters before `srcset` was condensed.
const DSL_CHAR_BUDGET: usize = 100_000;
const REMOVED_DSL_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
];
const REMOVED_DSL_TAG_PREFIXES: &[&str] = &["header-", "footer-", "nav-"];

#[derive(Debug, Default, Serialize)]
struct PageDslRoot {
    page_dsl: PageDsl,
}

#[derive(Debug, Default, Serialize)]
struct PageDsl {
    /// How many elements, the last in document order, the projection left out to stay within
    /// [`DSL_CHAR_BUDGET`].
    #[serde(skip_serializing_if = "Option::is_none")]
    omitted_nodes: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    nodes: Vec<HtmlYamlNode>,
}

#[derive(Debug, Serialize)]
struct HtmlYamlNode {
    tag: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    attrs: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<HtmlYamlNode>,
}

pub fn html_to_schema_prompt_dsl(input: &str) -> String {
    let document = parse_html().one(input);

    for selector in REMOVED_DSL_TAGS {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }
    remove_comments(&document);

    let mut projected_count = 0;
    let nodes = project_children(&document, &mut projected_count);
    serialize_within_budget(nodes, DSL_CHAR_BUDGET)
}

/// The YAML of the longest document-order prefix of `nodes` within `budget` characters. A prefix
/// in document order keeps every kept element's ancestors, so the kept tree reads as the page
/// does and its selectors stay derivable.
fn serialize_within_budget(nodes: Vec<HtmlYamlNode>, budget: usize) -> String {
    let total = count_nodes(&nodes);
    let page_of_prefix = |kept: usize| {
        let mut remaining = kept;
        serialize_page(PageDsl {
            omitted_nodes: (kept < total).then_some(total - kept),
            nodes: document_order_prefix(&nodes, &mut remaining),
        })
    };

    let whole = page_of_prefix(total);
    if whole.chars().count() <= budget {
        return whole;
    }
    // The YAML grows with every element kept, so the longest prefix within the budget is found by
    // bisecting the number of elements kept.
    let (mut fits, mut exceeds) = (0, total);
    while exceeds - fits > 1 {
        let middle = fits + (exceeds - fits) / 2;
        if page_of_prefix(middle).chars().count() <= budget {
            fits = middle;
        } else {
            exceeds = middle;
        }
    }
    page_of_prefix(fits)
}

fn serialize_page(page_dsl: PageDsl) -> String {
    yaml_serde::to_string(&PageDslRoot { page_dsl })
        .unwrap_or_else(|_| "page_dsl: {}\n".to_string())
}

fn count_nodes(nodes: &[HtmlYamlNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + count_nodes(&node.children))
        .sum()
}

/// The first `remaining` elements of `nodes` in document order, each parent before its children.
fn document_order_prefix(nodes: &[HtmlYamlNode], remaining: &mut usize) -> Vec<HtmlYamlNode> {
    let mut prefix = Vec::new();
    for node in nodes {
        if *remaining == 0 {
            break;
        }
        *remaining -= 1;
        prefix.push(HtmlYamlNode {
            tag: node.tag.clone(),
            attrs: node.attrs.clone(),
            text: node.text.clone(),
            children: document_order_prefix(&node.children, remaining),
        });
    }
    prefix
}

fn project_children(node: &NodeRef, projected_count: &mut usize) -> Vec<HtmlYamlNode> {
    let mut projected = Vec::new();
    for child in node.children() {
        if *projected_count >= DSL_NODE_LIMIT {
            break;
        }
        projected.extend(project_node(&child, projected_count));
    }
    projected
}

fn project_node(node: &NodeRef, projected_count: &mut usize) -> Vec<HtmlYamlNode> {
    let Some(element) = node.as_element() else {
        return Vec::new();
    };

    let tag = element.name.local.to_string();
    if is_removed_dsl_tag(&tag) {
        return Vec::new();
    }

    let attrs = {
        let attrs = element.attributes.borrow();
        projected_attrs(node, &tag, &attrs)
    };
    let text = projected_text(node, &attrs).map(|text| truncate_chars(&text, DSL_TEXT_LIMIT));
    let children = project_children(node, projected_count);

    if attrs.is_empty() && text.is_none() && children.is_empty() {
        return Vec::new();
    }

    if is_collapsible_wrapper(&tag)
        && text.is_none()
        && is_layout_only_attrs(&attrs)
        && !has_product_specific_context(&attrs)
    {
        return children;
    }

    *projected_count += 1;
    vec![HtmlYamlNode {
        tag,
        attrs,
        text,
        children,
    }]
}

fn is_removed_dsl_tag(tag: &str) -> bool {
    REMOVED_DSL_TAGS.contains(&tag)
        || REMOVED_DSL_TAG_PREFIXES
            .iter()
            .any(|prefix| tag.starts_with(prefix))
}

fn is_collapsible_wrapper(tag: &str) -> bool {
    matches!(
        tag,
        "html" | "head" | "body" | "main" | "div" | "span" | "section" | "article"
    )
}

fn is_layout_only_attrs(attrs: &BTreeMap<String, String>) -> bool {
    attrs.is_empty()
        || (attrs.len() == 1 && attrs.contains_key("class"))
        || attrs
            .keys()
            .all(|key| matches!(key.as_str(), "class" | "id"))
}

fn has_product_specific_context(attrs: &BTreeMap<String, String>) -> bool {
    const CONTEXT_TERMS: &[&str] = &[
        "product",
        "price",
        "gallery",
        "image",
        "photo",
        "sku",
        "availability",
        "description",
        "seller",
        "auction",
        "lot",
        "details",
        "summary",
    ];

    ["class", "id"]
        .iter()
        .filter_map(|name| attrs.get(*name))
        .map(|value| value.to_ascii_lowercase())
        .any(|value| CONTEXT_TERMS.iter().any(|term| value.contains(term)))
}

fn projected_attrs(
    node: &NodeRef,
    tag: &str,
    attrs: &kuchiki::Attributes,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for name in PROJECTED_ATTRS {
        let Some(raw_value) = attrs.get(*name) else {
            continue;
        };
        let value = if raw_value.trim().is_empty() && is_boolean_projected_attr(name) {
            "true"
        } else {
            raw_value
        };
        if value.trim().is_empty() {
            continue;
        }
        if !should_project_attr(node, tag, attrs, name, value) {
            continue;
        }
        result.insert((*name).to_string(), projected_attr_value(name, value));
    }
    result
}

fn is_boolean_projected_attr(name: &str) -> bool {
    matches!(name, "itemscope")
}

fn should_project_attr(
    node: &NodeRef,
    tag: &str,
    attrs: &kuchiki::Attributes,
    name: &str,
    value: &str,
) -> bool {
    match name {
        "href" => {
            has_known_image_extension(value)
                || is_image_like_anchor_context(node, tag, attrs, value)
        }
        "rel" => tag == "a",
        _ => true,
    }
}

fn is_image_like_anchor_context(
    node: &NodeRef,
    tag: &str,
    attrs: &kuchiki::Attributes,
    href: &str,
) -> bool {
    tag == "a"
        && (anchor_wraps_image(node)
            || attrs_have_image_context(attrs)
            || path_has_image_context(href))
}

fn anchor_wraps_image(node: &NodeRef) -> bool {
    node.descendants().any(|descendant| {
        descendant
            .as_element()
            .is_some_and(|element| element.name.local.as_ref() == "img")
    })
}

fn attrs_have_image_context(attrs: &kuchiki::Attributes) -> bool {
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
        attrs.get("class"),
        attrs.get("id"),
        attrs.get("rel"),
        attrs.get("data-gallery"),
        attrs.get("data-lightbox"),
        attrs.get("data-fancybox"),
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

fn projected_attr_value(name: &str, value: &str) -> String {
    match name {
        "srcset" | "data-srcset" => condensed_srcset(value),
        _ => truncate_chars(value, projected_attr_limit(name)),
    }
}

/// A `srcset` of several candidates as its widest candidate by the `w` descriptor, the last of
/// equally wide ones, verbatim, and the number of the others: the candidate image extraction
/// picks from a `srcset`. Its URL is a substring of the page's attribute, so a selector such as
/// `img[srcset*="…"]` written from the projection matches the page. A `data:` URI, a lazy
/// loader's placeholder, is no candidate list: its commas belong to the URI.
fn condensed_srcset(value: &str) -> String {
    if is_data_uri(value) {
        return truncate_chars(value.trim(), DSL_ATTR_LIMIT);
    }
    let candidates = value
        .split(',')
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
        .collect::<Vec<_>>();
    let Some(largest) = candidates
        .iter()
        .max_by_key(|candidate| srcset_candidate_width(candidate))
    else {
        return truncate_chars(value, DSL_IMAGE_ATTR_LIMIT);
    };
    let largest = truncate_chars(largest, DSL_IMAGE_ATTR_LIMIT);
    match candidates.len() - 1 {
        0 => largest,
        others => format!("{largest} (+{others} more srcset candidates)"),
    }
}

fn is_data_uri(value: &str) -> bool {
    value
        .trim_start()
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
}

/// The width of a trimmed `srcset` candidate, its URL followed by its descriptors, as image
/// extraction reads it.
fn srcset_candidate_width(candidate: &str) -> usize {
    candidate
        .split_once(char::is_whitespace)
        .map_or(0, |(_, descriptors)| srcset_descriptor_width(descriptors))
}

fn projected_attr_limit(name: &str) -> usize {
    match name {
        "href" | "src" | "content" | "data-lazy" | "data-lazy-src" | "data-src"
        | "data-large_image" | "data-full" | "data-original" | "data-zoom-image" | "data-image"
        | "data-images" => DSL_IMAGE_ATTR_LIMIT,
        _ => DSL_ATTR_LIMIT,
    }
}

const PROJECTED_ATTRS: &[&str] = &[
    "id",
    "class",
    "itemscope",
    "itemtype",
    "itemprop",
    "property",
    "name",
    "content",
    "value",
    "type",
    "href",
    "rel",
    "role",
    "aria-label",
    "aria-labelledby",
    "aria-describedby",
    "src",
    "srcset",
    "alt",
    "title",
    "datetime",
    "data-lazy",
    "data-lazy-src",
    "data-src",
    "data-srcset",
    "data-large_image",
    "data-full",
    "data-original",
    "data-zoom-image",
    "data-id",
    "data-variant-id",
    "data-product",
    "data-product-id",
    "data-sku",
    "data-price",
    "data-currency",
    "data-availability",
    "data-image",
    "data-images",
    "data-gallery",
    "data-lightbox",
    "data-fancybox",
    "data-testid",
    "data-test",
    "data-cy",
];

fn normalize_text(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

fn direct_text(node: &NodeRef) -> Option<String> {
    let mut text = String::new();
    for child in node.children() {
        if let Some(contents) = child.as_text() {
            text.push_str(&contents.borrow());
            text.push(' ');
        }
    }
    normalize_text(&text)
}

fn descendant_text(node: &NodeRef) -> Option<String> {
    let mut text = String::new();
    for descendant in node.descendants() {
        if let Some(contents) = descendant.as_text() {
            text.push_str(&contents.borrow());
            text.push(' ');
        }
    }
    normalize_text(&text)
}

fn projected_text(node: &NodeRef, attrs: &BTreeMap<String, String>) -> Option<String> {
    if has_product_specific_context(attrs) {
        descendant_text(node)
    } else {
        direct_text(node)
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }

    let mut truncated = value.chars().take(limit).collect::<String>();
    truncated.push('…');
    truncated
}

fn remove_comments(node: &NodeRef) {
    for child in node.children() {
        if child.as_comment().is_some() {
            child.detach();
        } else {
            remove_comments(&child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::widest_by_w_descriptor(
        "/p-150.jpg 150w, /p-1200.jpg 1200w, /p-768.jpg 768w",
        "/p-1200.jpg 1200w (+2 more srcset candidates)"
    )]
    #[case::last_of_equally_wide_as_extraction_picks(
        "/p.jpg 1x, /p@2x.jpg 2x",
        "/p@2x.jpg 2x (+1 more srcset candidates)"
    )]
    #[case::data_uri_placeholder_kept_whole(
        "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
        "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7"
    )]
    #[case::single_candidate_unchanged("/image/abc 1200w", "/image/abc 1200w")]
    #[case::empty_candidates_ignored(
        " , /a.jpg 100w,, /b.jpg 200w ,",
        "/b.jpg 200w (+1 more srcset candidates)"
    )]
    fn should_condense_srcset_to_its_largest_candidate_and_count(
        #[case] srcset: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(condensed_srcset(srcset), expected);
    }

    #[test]
    fn should_condense_srcset_and_data_srcset_attributes_in_the_projection() {
        let candidates = (1..=12)
            .map(|step| {
                format!(
                    "https://cdn.example.com/oak-chair.jpg?width={0} {0}w",
                    step * 200
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let html = format!(
            r#"<main><img class="product-image" src="/oak-chair.jpg" srcset="{candidates}">
            <img class="product-image lazy" data-srcset="{candidates}"></main>"#
        );

        let dsl = html_to_schema_prompt_dsl(&html);

        let condensed =
            "https://cdn.example.com/oak-chair.jpg?width=2400 2400w (+11 more srcset candidates)";
        assert!(dsl.contains(&format!("srcset: {condensed}")), "{dsl}");
        assert!(dsl.contains(&format!("data-srcset: {condensed}")), "{dsl}");
        assert!(!dsl.contains("width=1200"), "{dsl}");
    }

    #[test]
    fn should_keep_a_selector_written_from_the_condensed_srcset_matching_the_page() {
        let html = r#"<main>
            <img class="thumb" srcset="/files/logo-100.png 100w, /files/logo-200.png 200w">
            <img class="thumb" srcset="/files/vase-480.webp 480w, /files/vase-1080.webp 1080w, /files/vase-2048.webp 2048w">
            </main>"#;
        let dsl = html_to_schema_prompt_dsl(html);
        let projected_url = dsl
            .lines()
            .filter_map(|line| line.trim().strip_prefix("srcset: "))
            .find_map(|value| {
                value
                    .split_whitespace()
                    .next()
                    .filter(|url| url.contains("vase"))
            })
            .expect("the vase srcset is projected");

        let page = scraper::Html::parse_document(html);
        let selector =
            scraper::Selector::parse(&format!(r#"img[srcset*="{projected_url}"]"#)).unwrap();
        let matched = page.select(&selector).collect::<Vec<_>>();

        assert_eq!(projected_url, "/files/vase-2048.webp");
        assert_eq!(matched.len(), 1);
        assert!(
            matched[0]
                .value()
                .attr("srcset")
                .unwrap()
                .contains("vase-480")
        );
    }

    #[test]
    fn should_keep_the_document_order_prefix_with_its_ancestors_and_name_the_rest_when_over_budget()
    {
        let items = (0..50)
            .map(|item| format!(r#"<li class="item">Item number {item}</li>"#))
            .collect::<String>();
        let html = format!(
            r#"<main><h1 class="product-title">Oak chair</h1><ul class="list">{items}</ul></main>"#
        );
        let project = || project_children(&parse_html().one(html.as_str()), &mut 0);

        let whole = serialize_within_budget(project(), usize::MAX);
        let dsl = serialize_within_budget(project(), 1_000);

        assert!(!whole.contains("omitted_nodes"));
        assert!(whole.contains("Item number 49"));
        assert!(dsl.chars().count() <= 1_000, "{dsl}");
        assert!(dsl.contains("Oak chair"));
        assert!(dsl.contains("class: list"));
        assert!(dsl.contains("Item number 0"));
        assert!(!dsl.contains("Item number 49"));
        let omitted = dsl
            .lines()
            .find_map(|line| line.trim().strip_prefix("omitted_nodes: "))
            .and_then(|count| count.parse::<usize>().ok())
            .expect("the projection names what it left out");
        let kept_items = dsl.matches("Item number").count();
        assert_eq!(omitted, 50 - kept_items);
    }

    #[test]
    fn should_project_an_image_heavy_page_within_the_budget_with_its_product_content() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/html/nicholaswells_available.html");
        let html = std::fs::read_to_string(&path).expect("the fixture page is readable");
        assert!(html.matches("srcset=").count() > 150);

        let dsl = html_to_schema_prompt_dsl(&html);

        assert!(dsl.chars().count() <= DSL_CHAR_BUDGET);
        assert!(
            dsl.contains("omitted_nodes:"),
            "the page exceeds the budget"
        );
        for product_content in [
            "Borghese Vase",
            "€11.327,95",
            "This distinguished Borghese Vase",
            "borghese-vase-grand-tour.webp",
        ] {
            assert!(dsl.contains(product_content), "missing {product_content}");
        }
        let srcset_chars = dsl
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                line.starts_with("srcset:") || line.starts_with("data-srcset:")
            })
            .map(str::len)
            .sum::<usize>();
        assert!(
            srcset_chars * 10 < dsl.len(),
            "srcset takes {srcset_chars} characters"
        );
    }
}
