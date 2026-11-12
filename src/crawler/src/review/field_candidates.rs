//! A page's field candidates as the review agents read them (`inspect_page` view `FIELDS`): per
//! product-schema field the few elements of the page's own content that could hold its value,
//! each in CSS notation with what it shows, and the values of the page's JSON-LD `Product`. It is
//! a fraction of the outline's size, so an author writes a schema from it and opens the outline
//! only for what it does not show. Like every projection it decides nothing: it is information
//! for whoever inspects the page (`docs/crawler/extraction-and-review.md`, review agents).

use crate::review::page_content_checks::StateReading;
use crate::review::product_markup::{
    JsonLdProductReference, json_ld_product_reference, json_ld_scripts,
};
use crate::scraper::css_selector::rule::{
    JsonLdOfferProperty, JsonLdProductProperty, json_ld_offer_value, json_ld_product_value,
};
use crate::scraper::plausibility::is_price_text;
use scraper::{ElementRef, Html};
use std::collections::BTreeMap;
use std::fmt::Write;

/// Elements outside the page's own content: site chrome and other products' teasers. Unlike the
/// price signal, the digest keeps the page's cart form, which holds its state and product ID.
const CHROME_TAGS: [&str; 7] = [
    "nav", "header", "footer", "aside", "script", "style", "noscript",
];
const OTHER_PRODUCTS_WORDS: [&str; 7] = [
    "related",
    "similar",
    "recommend",
    "upsell",
    "cross-sell",
    "carousel",
    "minicart",
];

/// The most candidates shown per field, so the digest stays a few thousand characters.
const MAX_CANDIDATES: usize = 5;
/// The longest value shown; a longer one is cut and its length named.
const MAX_VALUE_CHARS: usize = 90;
/// The longest text a label candidate holds: a status line or a button, not a paragraph.
const MAX_LABEL_CHARS: usize = 60;
const MIN_PROSE_CHARS: usize = 80;

/// The field candidates of `raw_html`. Callers on the async runtime run it on a blocking thread.
pub(crate) fn field_candidates(raw_html: &str) -> String {
    let html = Html::parse_document(raw_html);
    let mut fields = Fields::default();
    for element in html
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|element| is_own_content(*element))
    {
        fields.consider(element);
    }
    fields.render(&json_ld_values(&html), &json_ld_product_reference(&html))
}

fn is_own_content(element: ElementRef<'_>) -> bool {
    std::iter::once(element)
        .chain(element.ancestors().filter_map(ElementRef::wrap))
        .all(|candidate| {
            !CHROME_TAGS.contains(&candidate.value().name()) && {
                let names = names_of(candidate);
                !OTHER_PRODUCTS_WORDS.iter().any(|word| names.contains(word))
            }
        })
}

#[derive(Default)]
struct Fields {
    title: Vec<String>,
    price: Vec<String>,
    state: Vec<String>,
    source_listing_id: Vec<String>,
    description: Vec<String>,
    /// Images per container selector, with the first image's URL.
    images: BTreeMap<String, (usize, String)>,
}

impl Fields {
    fn consider(&mut self, element: ElementRef<'_>) {
        let value = element.value();
        let tag = value.name();
        let text = collapsed_text(element);
        let names = names_of(element);
        if tag == "h1" || (names.contains("title") && is_heading(tag)) {
            push(&mut self.title, element, &text);
        }
        if is_leaf_like(element) && text.chars().count() <= MAX_LABEL_CHARS {
            if is_price_text(&text) || (names.contains("price") && has_digit(&text)) {
                push(&mut self.price, element, &text);
            }
            let reading = StateReading::of(&text);
            let names_status = ["stock", "availab", "sold", "status", "badge"]
                .iter()
                .any(|word| names.contains(word));
            if reading.resolved.is_some() || reading.mentioned.is_some() || names_status {
                push(&mut self.state, element, &text);
            }
        }
        if is_action(element) {
            push(&mut self.state, element, &action_label(element, &text));
        }
        if let Some(availability) = value
            .attr("href")
            .filter(|_| names.contains("availability"))
        {
            push(&mut self.state, element, availability);
        }
        if let Some(id) = identity_value(element) {
            push(&mut self.source_listing_id, element, &id);
        }
        if (names.contains("description") || tag == "article")
            && text.chars().count() >= MIN_PROSE_CHARS
        {
            push(&mut self.description, element, &text);
        }
        if tag == "img"
            && let Some(source) = ["src", "data-src", "srcset"]
                .iter()
                .find_map(|attribute| value.attr(attribute))
            && let Some(container) = named_ancestor(element)
        {
            let entry = self
                .images
                .entry(format!("{container} img"))
                .or_insert_with(|| (0, shortened(source)));
            entry.0 += 1;
        }
    }

    fn render(&self, json_ld: &[(String, String)], reference: &JsonLdProductReference) -> String {
        let mut out = String::new();
        section(&mut out, "title", &self.title);
        section(&mut out, "price", &self.price);
        section(&mut out, "state", &self.state);
        section(&mut out, "source_listing_id", &self.source_listing_id);
        section(&mut out, "description", &self.description);
        let mut galleries: Vec<_> = self.images.iter().collect();
        galleries.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
        let images: Vec<String> = galleries
            .into_iter()
            .take(MAX_CANDIDATES)
            .map(|(selector, (count, first))| format!("{selector} ×{count} | {first}"))
            .collect();
        section(&mut out, "images", &images);
        let mut structured: Vec<String> = json_ld
            .iter()
            .map(|(rule, value)| format!("{rule} | {}", shortened(value)))
            .collect();
        if let Some(description) = &reference.description {
            structured.push(format!(
                "json_ld description ({} chars, a reference, not a rule) | {}",
                description.chars().count(),
                shortened(description)
            ));
        }
        if reference.image_count > 0 {
            structured.push(format!(
                "json_ld image count (a reference, not a rule) | {}",
                reference.image_count
            ));
        }
        section(&mut out, "json_ld", &structured);
        out
    }
}

fn push(list: &mut Vec<String>, element: ElementRef<'_>, value: &str) {
    if list.len() >= MAX_CANDIDATES || value.trim().is_empty() {
        return;
    }
    let line = format!("{} | {}", notation(element), shortened(value));
    if !list.contains(&line) {
        list.push(line);
    }
}

fn section(out: &mut String, field: &str, lines: &[String]) {
    let _ = writeln!(out, "{field}:");
    if lines.is_empty() {
        let _ = writeln!(out, "  (none found)");
    }
    for line in lines {
        let _ = writeln!(out, "  {line}");
    }
}

/// The values the `json_ld_offer` and `json_ld_product` kinds read on the page, each named by the
/// rule that reads it.
fn json_ld_values(html: &Html) -> Vec<(String, String)> {
    let scripts: Vec<String> = json_ld_scripts(html).collect();
    let mut values = Vec::new();
    for (name, property) in [
        ("price", JsonLdOfferProperty::Price),
        ("availability", JsonLdOfferProperty::Availability),
    ] {
        if let Some(value) = scripts
            .iter()
            .find_map(|script| json_ld_offer_value(script, property))
        {
            values.push((format!("json_ld_offer {name}"), value));
        }
    }
    for (name, property) in [
        ("product_id", JsonLdProductProperty::ProductId),
        ("sku", JsonLdProductProperty::Sku),
    ] {
        if let Some(value) = scripts
            .iter()
            .find_map(|script| json_ld_product_value(script, property))
        {
            values.push((format!("json_ld_product {name}"), value));
        }
    }
    values
}

/// The element in CSS notation: its tag, id, classes and the attributes that name what it is.
fn notation(element: ElementRef<'_>) -> String {
    let value = element.value();
    let mut notation = value.name().to_owned();
    if let Some(id) = value.id().filter(|id| is_css_identifier(id)) {
        let _ = write!(notation, "#{id}");
    }
    for class in classes(element).filter(|class| is_css_identifier(class)) {
        let _ = write!(notation, ".{class}");
    }
    for attribute in ["itemprop", "name", "type", "data-product-id", "data-id"] {
        if let Some(attribute_value) = value.attr(attribute) {
            let _ = write!(
                notation,
                "[{attribute}='{}']",
                attribute_value.replace('\'', "\\'")
            );
        }
    }
    notation
}

/// The nearest ancestor of an image that names itself by an id or a class, as a gallery selector.
fn named_ancestor(element: ElementRef<'_>) -> Option<String> {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .find(|ancestor| {
            ancestor.value().id().is_some_and(is_css_identifier)
                || classes(*ancestor).any(is_css_identifier)
        })
        .map(|ancestor| {
            let value = ancestor.value();
            match value.id().filter(|id| is_css_identifier(id)) {
                Some(id) => format!("#{id}"),
                None => classes(ancestor)
                    .filter(|class| is_css_identifier(class))
                    .map(|class| format!(".{class}"))
                    .collect(),
            }
        })
}

/// A product ID an attribute holds: a hidden input or cart button naming the product, a
/// `data-product-id` or a microdata `sku` or `productID`.
fn identity_value(element: ElementRef<'_>) -> Option<String> {
    let value = element.value();
    let name = value.attr("name").unwrap_or_default().to_ascii_lowercase();
    let itemprop = value.attr("itemprop").unwrap_or_default();
    if matches!(value.name(), "input" | "button")
        && (name.contains("product") || name == "add-to-cart" || name == "id")
    {
        return value.attr("value").map(str::to_owned);
    }
    if let Some(id) = value.attr("data-product-id") {
        return Some(id.to_owned());
    }
    if matches!(itemprop, "sku" | "productID") {
        return value
            .attr("content")
            .map(str::to_owned)
            .or_else(|| Some(collapsed_text(element)));
    }
    None
}

/// The element's classes in document order, so a notation reads as the page writes it.
fn classes(element: ElementRef<'_>) -> impl Iterator<Item = &str> {
    element
        .value()
        .attr("class")
        .unwrap_or_default()
        .split_whitespace()
}

fn is_action(element: ElementRef<'_>) -> bool {
    let value = element.value();
    match value.name() {
        "button" => value.attr("type") != Some("button") || names_of(element).contains("cart"),
        "input" => matches!(value.attr("type"), Some("submit")),
        _ => false,
    }
}

fn action_label(element: ElementRef<'_>, text: &str) -> String {
    if text.is_empty() {
        element.value().attr("value").unwrap_or_default().to_owned()
    } else {
        text.to_owned()
    }
}

fn is_heading(tag: &str) -> bool {
    matches!(tag, "h1" | "h2" | "h3")
}

/// Whether the element holds text of its own rather than only other elements' text.
fn is_leaf_like(element: ElementRef<'_>) -> bool {
    element.children().filter_map(ElementRef::wrap).count() <= 2
}

fn has_digit(text: &str) -> bool {
    text.chars().any(|character| character.is_ascii_digit())
}

fn names_of(element: ElementRef<'_>) -> String {
    ["class", "id", "itemprop"]
        .into_iter()
        .filter_map(|attribute| element.value().attr(attribute))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn collapsed_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn shortened(value: &str) -> String {
    let count = value.chars().count();
    if count <= MAX_VALUE_CHARS {
        return value.to_owned();
    }
    let start: String = value.chars().take(MAX_VALUE_CHARS).collect();
    format!("{start}… ({count} chars)")
}

fn is_css_identifier(name: &str) -> bool {
    let rest = name.strip_prefix('-').unwrap_or(name);
    rest.chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && rest
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><script type="application/ld+json">{"@context":"https://schema.org","@type":"Product","name":"Oak chair","sku":"OC-1","description":"A Victorian oak chair.","image":["a.jpg","b.jpg"],"offers":{"price":"120.00","priceCurrency":"GBP","availability":"https://schema.org/InStock"}}</script></head>
        <body><nav><a>Menu £5 offer</a></nav><div class="product"><h1 class="product_title">Oak chair</h1>
        <div class="gallery"><img src="https://shop.example/a.jpg"><img src="https://shop.example/b.jpg"></div>
        <p class="price">£120.00</p><p class="stock in-stock">1 in stock</p>
        <form class="cart"><input type="hidden" name="product_id" value="4411"><button type="submit" class="single_add_to_cart_button">Add to cart</button></form>
        <div class="product-description"><p>A Victorian oak chair with a carved back rail, turned front legs and its original finish, circa 1880.</p></div></div></body></html>"#;

    #[test]
    fn should_name_each_field_s_candidates_in_css_notation_with_their_values() {
        let digest = field_candidates(PAGE);

        for expected in [
            "h1.product_title | Oak chair",
            "p.price | £120.00",
            "p.stock.in-stock | 1 in stock",
            "button.single_add_to_cart_button[type='submit'] | Add to cart",
            "input[name='product_id'][type='hidden'] | 4411",
            "div.product-description | A Victorian oak chair",
            ".gallery img ×2 | https://shop.example/a.jpg",
            "json_ld_offer price | 120.00 GBP",
            "json_ld_product sku | OC-1",
            "json_ld image count (a reference, not a rule) | 2",
        ] {
            assert!(
                digest.contains(expected),
                "{expected:?} missing from\n{digest}"
            );
        }
        assert!(!digest.contains("Menu"), "{digest}");
    }

    #[test]
    fn should_stay_a_fraction_of_a_long_page() {
        let filler = "<div class=\"row\"><span>Item</span></div>".repeat(2_000);
        let page = PAGE.replace(
            "<div class=\"product\">",
            &format!("{filler}<div class=\"product\">"),
        );

        assert!(field_candidates(&page).len() < 3_000);
    }
}
