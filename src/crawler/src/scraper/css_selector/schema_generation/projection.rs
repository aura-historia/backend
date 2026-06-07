use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use serde::Serialize;
use std::collections::BTreeMap;

pub fn clean_html_for_schema_generation(input: &str) -> String {
    let document = parse_html().one(input);

    // Tags to remove entirely
    let remove_selectors = [
        "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
    ];

    for selector in &remove_selectors {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }

    remove_comments(&document);
    strip_attributes(&document);
    let mut cleaned = Vec::new();
    document.serialize(&mut cleaned).unwrap();
    String::from_utf8(cleaned).unwrap_or_default()
}

const DSL_TEXT_LIMIT: usize = 180;
const DSL_ATTR_LIMIT: usize = 250;
const DSL_IMAGE_ATTR_LIMIT: usize = 1_000;
const DSL_NODE_LIMIT: usize = 2_000;
const REMOVED_DSL_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
];

#[derive(Debug, Default, Serialize)]
struct PageDslRoot {
    page_dsl: PageDsl,
}

#[derive(Debug, Default, Serialize)]
struct PageDsl {
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
    let page = PageDsl { nodes };

    yaml_serde::to_string(&PageDslRoot { page_dsl: page })
        .unwrap_or_else(|_| "page_dsl: {}\n".to_string())
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
    if REMOVED_DSL_TAGS.contains(&tag.as_str()) {
        return Vec::new();
    }

    let attrs = {
        let attrs = element.attributes.borrow();
        projected_attrs(node, &tag, &attrs)
    };
    let text = direct_text(node).map(|text| truncate_chars(&text, DSL_TEXT_LIMIT));
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
        result.insert(
            (*name).to_string(),
            truncate_chars(value, projected_attr_limit(name)),
        );
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

fn has_known_image_extension(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".png")
        || lower.contains(".webp")
        || lower.contains(".gif")
        || lower.contains(".avif")
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

fn projected_attr_limit(name: &str) -> usize {
    match name {
        "href" | "src" | "srcset" | "content" | "data-lazy" | "data-lazy-src" | "data-src"
        | "data-srcset" | "data-large_image" | "data-full" | "data-original"
        | "data-zoom-image" | "data-image" | "data-images" => DSL_IMAGE_ATTR_LIMIT,
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

fn strip_attributes(document: &NodeRef) {
    let deny_prefixes = ["on"]; // onclick, onload, etc.

    let deny_exact = [
        "style",
        "integrity",
        "crossorigin",
        "referrerpolicy",
        "nonce",
        "tabindex",
        "width",
        "height",
        "loading",
        "decoding",
    ];

    for css_match in document.select("*").unwrap() {
        let mut attributes = css_match.attributes.borrow_mut();

        attributes.map.retain(|key, _| {
            let name = key.local.as_ref();

            // Remove JS event handlers
            if deny_prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                return false;
            }

            // Remove known useless attributes
            if deny_exact.contains(&name) {
                return false;
            }

            true
        });
    }
}
