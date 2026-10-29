//! Whether a fetched page is the page of one product by its own structured data: a schema.org
//! `Product` in JSON-LD or microdata, parsed deterministically. It is information for whoever
//! inspects a page, such as the review agents labelling a URL sample, and decides no approval
//! (`docs/crawler/extraction-and-review.md`, approval guard).

use scraper::{ElementRef, Html, Selector};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::sync::LazyLock;

static SCRIPTS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("script[type]").unwrap_or_else(|error| panic!("{error}")));

/// Whether `html` describes exactly one schema.org `Product` as a top-level item of its JSON-LD or
/// of its microdata, and no more than one in either. A JSON-LD item is top-level as a script's
/// node, a member of a top-level array or of a top-level node's `@graph`, with `@type` naming
/// `Product` as a string or in an array; nodes naming the same `url`, or else the same `@id`,
/// describe one product, as when a theme and a plugin both mark the page up. A microdata item is
/// top-level when no other item contains it, and its `itemtype` names schema.org's own `Product`
/// IRI. A product nested in another item, such as a list or a page, does not count, and a page that
/// lists several products top-level, such as a category page marking each product card, is no
/// product page. A script that is not valid JSON shows nothing.
pub fn describes_one_product(html: &str) -> bool {
    let document = Html::parse_document(html);
    let json_ld = json_ld_products(&document);
    let microdata = microdata_products(&document);
    (json_ld == 1 || microdata == 1) && json_ld <= 1 && microdata <= 1
}

/// How many distinct products the top-level JSON-LD `Product` nodes describe: one per `url`, or
/// else per `@id`, and one per node naming neither.
fn json_ld_products(document: &Html) -> usize {
    let scripts: Vec<Value> = document
        .select(&SCRIPTS)
        .filter(|script| script.attr("type").is_some_and(is_json_ld_type))
        .filter_map(|script| serde_json::from_str(&script.text().collect::<String>()).ok())
        .collect();
    let mut nodes = Vec::new();
    for value in &scripts {
        collect_top_level_products(value, &mut nodes);
    }
    let identified: BTreeSet<String> = nodes.iter().filter_map(|node| identity(node)).collect();
    let anonymous = nodes.iter().filter(|node| identity(node).is_none()).count();
    identified.len() + anonymous
}

/// What names the product a node describes: its `url`, or else its `@id`.
fn identity(node: &Map<String, Value>) -> Option<String> {
    ["url", "@id"].into_iter().find_map(|key| {
        node.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| format!("{key} {name}"))
    })
}

fn is_json_ld_type(media_type: &str) -> bool {
    media_type
        .split(';')
        .next()
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/ld+json"))
}

/// Collects the `Product` nodes of a JSON-LD value at its top level: the value itself, the members
/// of an array and the members of a node's `@graph`, never a node's other properties.
fn collect_top_level_products<'a>(value: &'a Value, products: &mut Vec<&'a Map<String, Value>>) {
    match value {
        Value::Array(nodes) => {
            for node in nodes {
                collect_top_level_products(node, products);
            }
        }
        Value::Object(node) => {
            if node.get("@type").is_some_and(names_product) {
                products.push(node);
            }
            if let Some(graph) = node.get("@graph") {
                collect_top_level_products(graph, products);
            }
        }
        _ => {}
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

/// schema.org's own address of `Product`: `http` or `https`, the host `schema.org` or
/// `www.schema.org`, and the path `/Product`, with or without one trailing slash. schema.org
/// publishes `https://schema.org/Product`; the other forms are spellings pages use for the same
/// term. Scheme and host match in any case, as URLs do; the term is case-sensitive, as schema.org's
/// terms are. Anything else, such as another host ending in `schema.org` or a path ending in
/// `schema.org/Product`, names another vocabulary's type.
fn is_product_iri(name: &str) -> bool {
    let Some((scheme, address)) = name.split_once("://") else {
        return false;
    };
    let Some((host, path)) = address.split_once('/') else {
        return false;
    };
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let schema_org =
        host.eq_ignore_ascii_case("schema.org") || host.eq_ignore_ascii_case("www.schema.org");
    web && schema_org && matches!(path, "Product" | "Product/")
}

/// How many microdata items inside no other item name `Product` in their `itemtype`. One pass
/// over the document tracks whether an item contains each node, so a deeply nested page, which
/// the shop controls, costs no more than its size; looking up each item's ancestors would cost
/// its depth for every item.
fn microdata_products(document: &Html) -> usize {
    let mut products = 0;
    let mut pending = vec![(document.tree.root(), false)];
    while let Some((node, inside_item)) = pending.pop() {
        let item = ElementRef::wrap(node).filter(|element| element.attr("itemscope").is_some());
        if !inside_item
            && item
                .and_then(|item| item.attr("itemtype"))
                .is_some_and(|types| types.split_ascii_whitespace().any(is_product_iri))
        {
            products += 1;
        }
        let inside_item = inside_item || item.is_some();
        pending.extend(node.children().map(|child| (child, inside_item)));
    }
    products
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

    /// A page whose one item is typed `iri`, once in microdata and once in JSON-LD.
    fn items_typed(iri: &str) -> [(&'static str, String); 2] {
        [
            (
                "microdata",
                page("", &format!(r#"<div itemscope itemtype="{iri}"></div>"#)),
            ),
            ("JSON-LD", json_ld(&format!(r#"{{"@type": "{iri}"}}"#))),
        ]
    }

    #[rstest::rstest]
    #[case::https("https://schema.org/Product")]
    #[case::http("http://schema.org/Product")]
    #[case::www_host("https://www.schema.org/Product")]
    #[case::trailing_slash("https://schema.org/Product/")]
    #[case::scheme_and_host_in_another_case("HTTPS://WWW.Schema.Org/Product/")]
    fn should_find_one_product_when_its_type_is_schema_orgs_product_iri(#[case] iri: &str) {
        for (markup, html) in items_typed(iri) {
            assert!(describes_one_product(&html), "{markup} {iri}");
        }
    }

    /// Only schema.org's own address names its `Product`: a look-alike host or a path that merely
    /// ends in `schema.org/Product` names another vocabulary's type.
    #[rstest::rstest]
    #[case::look_alike_host("https://notschema.org/Product")]
    #[case::host_under_another_domain("https://schema.org.example.com/Product")]
    #[case::subdomain("https://shop.schema.org/Product")]
    #[case::schema_org_as_user_info("https://schema.org@example.com/Product")]
    #[case::schema_org_in_the_path("https://example.com/schema.org/Product")]
    #[case::schema_org_in_the_query("https://example.com/?type=https://schema.org/Product")]
    #[case::percent_encoded_host("https://schema%2Eorg/Product")]
    #[case::product_in_a_deeper_path("https://schema.org/extensions/Product")]
    #[case::two_trailing_slashes("https://schema.org/Product//")]
    #[case::query("https://schema.org/Product?ref=shop")]
    #[case::term_in_lower_case("https://schema.org/product")]
    #[case::another_scheme("ftp://schema.org/Product")]
    #[case::scheme_relative("//schema.org/Product")]
    #[case::without_scheme("schema.org/Product")]
    fn should_find_no_product_when_its_type_is_an_iri_outside_schema_org(#[case] iri: &str) {
        for (markup, html) in items_typed(iri) {
            assert!(!describes_one_product(&html), "{markup} {iri}");
        }
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

    /// An item contains every element below it, however many plain elements lie between them.
    #[test]
    fn should_tell_top_level_from_nested_microdata_items_through_deep_markup() {
        let wrapped = |outer: &str| {
            let plain = 200;
            page(
                "",
                &format!(
                    "<div {outer}>{}<span itemscope itemtype=\"https://schema.org/Product\"></span>{}</div>",
                    "<div>".repeat(plain),
                    "</div>".repeat(plain)
                ),
            )
        };

        assert!(describes_one_product(&wrapped("class=\"page\"")));
        assert!(!describes_one_product(&wrapped(
            "itemscope itemtype=\"https://schema.org/WebPage\""
        )));
    }

    /// A theme and a plugin often both describe the page's product; nodes naming the same `url`,
    /// or else the same `@id`, describe one product.
    #[rstest::rstest]
    #[case::same_url(
        r#"[{"@type": "Product", "@id": "https://antiques.example/item/1", "url": "https://antiques.example/item/1"}, {"@type": "Product", "@id": "/item/1#product", "url": "https://antiques.example/item/1"}]"#
    )]
    #[case::same_id(
        r##"{"@graph": [{"@type": "Product", "@id": "#product"}, {"@type": "Product", "@id": "#product"}]}"##
    )]
    fn should_count_nodes_of_the_same_product_once(#[case] json: &str) {
        assert!(describes_one_product(&json_ld(json)), "{json}");
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

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/html")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()))
    }

    /// Real shop pages: product pages marked up in JSON-LD, a WooCommerce or Yoast `@graph`, a
    /// Shopify page whose theme and app both describe the product, or microdata, show their
    /// product; product pages without structured product markup show none.
    #[rstest::rstest]
    #[case::woocommerce_graph("antik-und-stil_available.html", true)]
    #[case::item_page_graph("georgianantiques_available.html", true)]
    #[case::shopify_theme_and_app("hickmet_poa.html", true)]
    #[case::shopify_product_group("nicholaswells_available.html", true)]
    #[case::single_node("proantic_available.html", true)]
    #[case::website_and_product("chairish_available.html", true)]
    #[case::microdata("weitze_available.html", true)]
    #[case::yoast_graph_without_product("collingeantiques_listed.html", false)]
    #[case::sold_page_without_product("collingeantiques_sold.html", false)]
    #[case::no_structured_data("lot-tissimo_listed.html", false)]
    fn should_read_the_product_markup_of_real_shop_pages(
        #[case] name: &str,
        #[case] product: bool,
    ) {
        assert_eq!(describes_one_product(&fixture(name)), product, "{name}");
    }
}
