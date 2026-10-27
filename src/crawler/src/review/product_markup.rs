//! Whether a fetched page is the page of one product by its own structured data: a schema.org
//! `Product` in JSON-LD or microdata, parsed deterministically. A URL-pattern review's live
//! evaluation counts such a page as a product page (`docs/crawler/extraction-and-review.md`,
//! approval guard).

use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;

static SCRIPTS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script[type]").unwrap_or_else(|error| panic!("{error}")));
static MICRODATA_ITEMS: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("[itemscope][itemtype]").unwrap_or_else(|error| panic!("{error}"))
});

/// Whether `html` describes exactly one schema.org `Product` as a top-level item of its JSON-LD or
/// of its microdata, and no more than one in either. A JSON-LD item is top-level as a script's
/// node, a member of a top-level array or of a top-level node's `@graph`, with `@type` naming
/// `Product` as a string or in an array; a microdata item is top-level when no other item
/// contains it, and its `itemtype` ends in `schema.org/Product`. A product nested in another item,
/// such as a list or a page, does not count, and a page that lists several products top-level,
/// such as a category page marking each product card, is no product page. A script that is not
/// valid JSON shows nothing.
pub(crate) fn describes_one_product(html: &str) -> bool {
    let document = Html::parse_document(html);
    let json_ld = json_ld_products(&document);
    let microdata = microdata_products(&document);
    (json_ld == 1 || microdata == 1) && json_ld <= 1 && microdata <= 1
}

fn json_ld_products(document: &Html) -> usize {
    document
        .select(&SCRIPTS)
        .filter(|script| script.attr("type").is_some_and(is_json_ld_type))
        .filter_map(|script| serde_json::from_str::<Value>(&script.text().collect::<String>()).ok())
        .map(|value| top_level_products(&value))
        .sum()
}

fn is_json_ld_type(media_type: &str) -> bool {
    media_type
        .split(';')
        .next()
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/ld+json"))
}

/// The `Product` nodes of a JSON-LD value at its top level: the value itself, the members of an
/// array and the members of a node's `@graph`, never a node's other properties.
fn top_level_products(value: &Value) -> usize {
    match value {
        Value::Array(nodes) => nodes.iter().map(top_level_products).sum(),
        Value::Object(node) => {
            let itself = usize::from(node.get("@type").is_some_and(names_product));
            itself + node.get("@graph").map_or(0, top_level_products)
        }
        _ => 0,
    }
}

fn names_product(types: &Value) -> bool {
    match types {
        Value::String(name) => is_product_type(name),
        Value::Array(names) => names
            .iter()
            .any(|name| name.as_str().is_some_and(is_product_type)),
        _ => false,
    }
}

/// `Product` in schema.org's vocabulary: the bare term, its compact `schema:` form or its IRI.
fn is_product_type(name: &str) -> bool {
    let name = name.trim();
    name == "Product" || name == "schema:Product" || is_product_iri(name)
}

fn is_product_iri(name: &str) -> bool {
    name.ends_with("schema.org/Product")
}

fn microdata_products(document: &Html) -> usize {
    document
        .select(&MICRODATA_ITEMS)
        .filter(|item| {
            item.attr("itemtype")
                .is_some_and(|types| types.split_ascii_whitespace().any(is_product_iri))
        })
        .filter(|item| !is_nested_item(*item))
        .count()
}

fn is_nested_item(item: ElementRef<'_>) -> bool {
    item.ancestors()
        .filter_map(ElementRef::wrap)
        .any(|ancestor| ancestor.attr("itemscope").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(head: &str, body: &str) -> String {
        format!("<!doctype html><html><head>{head}</head><body>{body}</body></html>")
    }

    fn json_ld(json: &str) -> String {
        page(
            &format!(r#"<script type="application/ld+json">{json}</script>"#),
            "<h1>Chair</h1>",
        )
    }

    #[rstest::rstest]
    #[case::bare_type(r#"{"@context": "https://schema.org", "@type": "Product", "name": "Chair"}"#)]
    #[case::type_iri(r#"{"@type": "http://schema.org/Product", "name": "Chair"}"#)]
    #[case::compact_type(
        r#"{"@context": {"schema": "https://schema.org/"}, "@type": "schema:Product"}"#
    )]
    #[case::type_array(r#"{"@type": ["Product", "Thing"], "name": "Chair"}"#)]
    #[case::in_graph(
        r#"{"@context": "https://schema.org", "@graph": [{"@type": "WebSite"}, {"@type": "BreadcrumbList"}, {"@type": "Product", "name": "Chair"}]}"#
    )]
    #[case::in_top_level_array(
        r#"[{"@type": "Organization"}, {"@type": "Product", "name": "Chair"}]"#
    )]
    #[case::graph_in_array(r#"[{"@graph": [{"@type": "Product", "name": "Chair"}]}]"#)]
    fn should_find_one_product_in_json_ld(#[case] json: &str) {
        assert!(describes_one_product(&json_ld(json)), "{json}");
    }

    #[test]
    fn should_find_one_product_in_json_ld_with_a_charset_parameter_and_surrounding_whitespace() {
        let html = page(
            "<script type=\" Application/LD+JSON; charset=utf-8 \">\n  {\"@type\": \"Product\"}\n</script>",
            "",
        );

        assert!(describes_one_product(&html));
    }

    #[rstest::rstest]
    #[case::https(r#"<div itemscope itemtype="https://schema.org/Product"><h1 itemprop="name">Chair</h1></div>"#)]
    #[case::http(r#"<div itemscope itemtype="http://schema.org/Product"><span itemprop="name">Chair</span></div>"#)]
    #[case::with_another_type(
        r#"<div itemscope itemtype="https://schema.org/Thing https://schema.org/Product"></div>"#
    )]
    #[case::with_a_nested_offer(
        r#"<div itemscope itemtype="https://schema.org/Product"><div itemprop="offers" itemscope itemtype="https://schema.org/Offer"></div></div>"#
    )]
    fn should_find_one_product_in_microdata(#[case] body: &str) {
        assert!(describes_one_product(&page("", body)), "{body}");
    }

    #[test]
    fn should_count_one_product_described_in_both_json_ld_and_microdata() {
        let html = page(
            r#"<script type="application/ld+json">{"@type": "Product"}</script>"#,
            r#"<div itemscope itemtype="https://schema.org/Product"></div>"#,
        );

        assert!(describes_one_product(&html));
    }

    /// Without a schema.org `Product`, with only a product nested in another item, or with several
    /// top-level products, a page shows no product of its own.
    #[rstest::rstest]
    #[case::no_markup(page("", "<h1>Chair</h1><span class=\"price\">£120</span>"))]
    #[case::other_type(json_ld(r#"{"@type": "WebPage", "name": "Chairs"}"#))]
    #[case::product_subtype_name(json_ld(r#"{"@type": "ProductGroup"}"#))]
    #[case::type_naming_product_as_a_word(json_ld(r#"{"@type": "Product Listing"}"#))]
    #[case::product_without_type(json_ld(r#"{"name": "Product"}"#))]
    #[case::nested_in_a_list(json_ld(
        r#"{"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "item": {"@type": "Product"}}]}"#
    ))]
    #[case::nested_as_main_entity(json_ld(
        r#"{"@type": "WebPage", "mainEntity": {"@type": "Product"}}"#
    ))]
    #[case::several_in_graph(json_ld(
        r#"{"@graph": [{"@type": "Product"}, {"@type": "Product"}]}"#
    ))]
    #[case::several_scripts(page(
        r#"<script type="application/ld+json">{"@type": "Product"}</script><script type="application/ld+json">{"@type": "Product"}</script>"#,
        ""
    ))]
    #[case::several_microdata_cards(page(
        "",
        r#"<div itemscope itemtype="https://schema.org/Product"></div><div itemscope itemtype="https://schema.org/Product"></div>"#
    ))]
    #[case::one_product_with_microdata_related_products(page(
        r#"<script type="application/ld+json">{"@type": "Product"}</script>"#,
        r#"<div itemscope itemtype="https://schema.org/Product"></div><div itemscope itemtype="https://schema.org/Product"></div>"#
    ))]
    #[case::microdata_nested_in_a_list(page(
        "",
        r#"<ul itemscope itemtype="https://schema.org/ItemList"><li itemprop="itemListElement" itemscope itemtype="https://schema.org/Product"></li></ul>"#
    ))]
    #[case::microdata_without_itemscope(page(
        "",
        r#"<div itemtype="https://schema.org/Product"></div>"#
    ))]
    #[case::microdata_of_another_vocabulary(page(
        "",
        r#"<div itemscope itemtype="https://example.com/Product-Listing"></div>"#
    ))]
    #[case::script_of_another_type(page(
        r#"<script type="application/json">{"@type": "Product"}</script>"#,
        ""
    ))]
    #[case::malformed_json_ld(json_ld(r#"{"@type": "Product", "name": "Chair""#))]
    #[case::malformed_and_valid_json_ld_of_no_product(page(
        r#"<script type="application/ld+json">{"@type": "Product",</script><script type="application/ld+json">{"@type": "Organization"}</script>"#,
        ""
    ))]
    fn should_find_no_product_page(#[case] html: String) {
        assert!(!describes_one_product(&html), "{html}");
    }

    /// A malformed script shows nothing, so a valid one next to it still decides.
    #[test]
    fn should_ignore_a_malformed_script_next_to_a_valid_product() {
        let html = page(
            r#"<script type="application/ld+json">{"@type": </script><script type="application/ld+json">{"@type": "Product"}</script>"#,
            "",
        );

        assert!(describes_one_product(&html));
    }
}
