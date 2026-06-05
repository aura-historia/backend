use kuchiki::traits::*;
use kuchiki::{parse_html, NodeRef};
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
    let product_json_ld_nodes = product_json_ld_nodes(&document);

    for selector in REMOVED_DSL_TAGS {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }
    remove_comments(&document);

    let mut projected_count = 0;
    let mut nodes = product_json_ld_nodes;
    nodes.extend(project_children(&document, &mut projected_count));
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

fn product_json_ld_nodes(document: &NodeRef) -> Vec<HtmlYamlNode> {
    let Ok(scripts) = document.select(r#"script[type="application/ld+json"]"#) else {
        return Vec::new();
    };

    scripts
        .filter_map(|script| {
            let raw = script.as_node().text_contents();
            let parsed = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
            product_json_ld_attrs(&parsed).map(|attrs| HtmlYamlNode {
                tag: "json_ld_product".to_string(),
                attrs,
                text: None,
                children: Vec::new(),
            })
        })
        .collect()
}

fn product_json_ld_attrs(value: &serde_json::Value) -> Option<BTreeMap<String, String>> {
    let product = find_product_json_ld(value)?;
    let mut attrs = BTreeMap::new();
    insert_json_ld_field(&mut attrs, "@type", product.get("@type"));
    insert_json_ld_field(&mut attrs, "sku", product.get("sku"));
    insert_json_ld_field(&mut attrs, "name", product.get("name"));
    insert_json_ld_field(&mut attrs, "image", product.get("image"));
    insert_json_ld_field(&mut attrs, "brand", product.get("brand"));
    insert_json_ld_field(&mut attrs, "seller", product.get("seller"));

    if let Some(offers) = product.get("offers") {
        insert_json_ld_field(
            &mut attrs,
            "offers.price",
            json_ld_nested_field(offers, "price"),
        );
        insert_json_ld_field(
            &mut attrs,
            "offers.priceCurrency",
            json_ld_nested_field(offers, "priceCurrency"),
        );
        insert_json_ld_field(
            &mut attrs,
            "offers.availability",
            json_ld_nested_field(offers, "availability"),
        );
    }

    if attrs.is_empty() { None } else { Some(attrs) }
}

fn find_product_json_ld(
    value: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    match value {
        serde_json::Value::Object(object) => {
            if json_ld_type_contains_product(object.get("@type")) {
                return Some(object);
            }
            if let Some(graph) = object.get("@graph")
                && let Some(product) = find_product_json_ld(graph)
            {
                return Some(product);
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_product_json_ld),
        _ => None,
    }
}

fn json_ld_type_contains_product(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::String(kind)) => kind.eq_ignore_ascii_case("product"),
        Some(serde_json::Value::Array(kinds)) => kinds.iter().any(|kind| {
            kind.as_str()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("product"))
        }),
        _ => false,
    }
}

fn json_ld_nested_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Option<&'a serde_json::Value> {
    match value {
        serde_json::Value::Object(object) => object.get(field),
        serde_json::Value::Array(items) => items
            .iter()
            .find_map(|item| json_ld_nested_field(item, field)),
        _ => None,
    }
}

fn insert_json_ld_field(
    attrs: &mut BTreeMap<String, String>,
    name: &str,
    value: Option<&serde_json::Value>,
) {
    let Some(value) = value.and_then(json_ld_value_to_string) else {
        return;
    };
    attrs.insert(
        name.to_string(),
        truncate_chars(&value, projected_attr_limit(name)),
    );
}

fn json_ld_value_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Array(values) => {
            let joined = values
                .iter()
                .filter_map(json_ld_value_to_string)
                .collect::<Vec<_>>()
                .join(", ");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        serde_json::Value::Object(object) => object
            .get("name")
            .or_else(|| object.get("@id"))
            .and_then(json_ld_value_to_string),
        _ => None,
    }
}

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
