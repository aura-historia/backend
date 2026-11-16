//! What a fetched page's own structured data says about the products on it, for the scraper's
//! product markup pre-check (`docs/crawler/extraction-and-review.md#product-markup-pre-check`).
//!
//! The verdict is stricter than the review console's
//! [`describes_one_product`](crate::review::product_markup::describes_one_product), which is
//! information for whoever inspects a page: it decides whether a page that fails every live
//! schema may be classified as no product page without a review, so it counts schema.org's
//! `Product` and every subtype of it, sees a product wherever the markup nests it, and calls a
//! page whose markup it cannot read unclear rather than empty. Only a page whose markup shows a
//! listing is classified; a page without any product markup, such as a removal notice answered
//! with status 200, is left to its review, where it can be declared removed.

use crate::review::product_markup::{json_ld_identity, json_ld_scripts};
use crate::scraper::css_selector::rule::{JsonLdContext, schema_org_term_of_iri};
use scraper::{ElementRef, Html};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

/// schema.org's `Product` and its subtypes: a node of any of these types is a product, such as the
/// `ProductGroup` a shop publishes for an item sold in several sizes.
const PRODUCT_TYPES: &[&str] = &[
    "Product",
    "ProductGroup",
    "ProductModel",
    "ProductCollection",
    "IndividualProduct",
    "SomeProducts",
    "Vehicle",
    "Car",
    "Motorcycle",
    "MotorizedBicycle",
    "BusOrCoach",
    "DietarySupplement",
    "Drug",
];

/// schema.org's web page types: a product a page item holds as its content is the page's own, as
/// one directly at the top level would be.
const PAGE_TYPES: &[&str] = &[
    "WebPage",
    "ItemPage",
    "CollectionPage",
    "SearchResultsPage",
    "AboutPage",
    "ContactPage",
    "CheckoutPage",
    "FAQPage",
    "QAPage",
    "ProfilePage",
    "MedicalWebPage",
    "RealEstateListing",
];

/// The fewest entries of an `ItemList` that make it a listing of items.
const MIN_LISTED_ITEMS: usize = 2;

/// What a page's structured data says about the products on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductMarkup {
    /// Exactly one product is the page's own: a product page.
    OneProduct,
    /// No product of any schema.org product type anywhere and no list of items, with every
    /// JSON-LD script read: the markup says nothing about the page.
    NoProduct,
    /// A listing: several products are the page's own, as on a category grid marking each card, or
    /// the page's own `ItemList` or `OfferCatalog` names several items, as a category page lists
    /// its products' URLs.
    Listing,
    /// Anything else: a JSON-LD script that does not parse, a single product the markup nests
    /// where it is not the page's own, or JSON-LD and microdata that disagree.
    Unclear,
}

impl ProductMarkup {
    /// Reads `html`'s JSON-LD and microdata.
    pub(crate) fn of_page(html: &str) -> Self {
        let document = Html::parse_document(html);
        let json_ld = match json_ld_products(&document) {
            Some(products) => products.verdict(),
            None => return Self::Unclear,
        };
        Self::combine(json_ld, microdata_products(&document).verdict())
    }

    /// Whether the markup shows that the page is no product page: it is a listing. A page without
    /// any product markup shows nothing.
    pub(crate) fn shows_a_listing(self) -> bool {
        self == Self::Listing
    }

    /// The page's verdict from those of its JSON-LD and its microdata, which describe one page: a
    /// side that names no product leaves the verdict to the other, and sides that disagree
    /// otherwise are unclear.
    fn combine(json_ld: Self, microdata: Self) -> Self {
        match (json_ld, microdata) {
            (verdict, Self::NoProduct) | (Self::NoProduct, verdict) => verdict,
            (Self::OneProduct, Self::OneProduct) => Self::OneProduct,
            (Self::Listing, Self::Listing) => Self::Listing,
            _ => Self::Unclear,
        }
    }
}

/// The products of one markup, outermost only: a product inside another product, such as a
/// `ProductGroup`'s variants or a product's related items, belongs to it and is not counted.
#[derive(Debug, Default)]
struct ProductCount {
    /// Distinct products that are the page's own: at its top level, in a `@graph`, or the main
    /// entity of a page item.
    own: usize,
    /// Products nested in other items, such as the members of an `ItemList`.
    nested: usize,
    /// The most items one of the page's own lists names (see [`listed_items`]).
    listed: usize,
}

impl ProductCount {
    fn verdict(&self) -> ProductMarkup {
        match (self.own, self.nested) {
            (1, _) => ProductMarkup::OneProduct,
            (0, _) if self.nested >= 2 || self.listed >= MIN_LISTED_ITEMS => ProductMarkup::Listing,
            (0, 0) => ProductMarkup::NoProduct,
            (0, _) => ProductMarkup::Unclear,
            _ => ProductMarkup::Listing,
        }
    }
}

/// Where a JSON-LD node sits on its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// The document, a member of a top-level array or of a top-level node's `@graph`.
    TopLevel,
    /// The content of a top-level page node, such as a `WebPage`'s `mainEntity`.
    PageContent,
    /// Anywhere else.
    Nested,
}

/// The products of the page's JSON-LD; `None` when a script does not parse, since the products it
/// may describe are then unknown.
fn json_ld_products(document: &Html) -> Option<ProductCount> {
    let documents = json_ld_scripts(document)
        .map(|script| serde_json::from_str::<Value>(script.trim()).ok())
        .collect::<Option<Vec<_>>>()?;
    let mut found = JsonLdProducts::default();
    for value in &documents {
        walk_json_ld(
            value,
            &JsonLdContext::initial(),
            Position::TopLevel,
            &mut found,
        );
    }
    let JsonLdProducts {
        own,
        nested,
        listed,
    } = found;
    let identified: BTreeSet<String> = own
        .iter()
        .filter_map(|node| json_ld_identity(node))
        .collect();
    let anonymous = own
        .iter()
        .filter(|node| json_ld_identity(node).is_none())
        .count();
    Some(ProductCount {
        own: identified.len() + anonymous,
        nested,
        listed,
    })
}

/// What a walk over a page's JSON-LD found: the page's own product nodes, the count of other
/// outermost product nodes, and the most items one of the page's own lists names.
#[derive(Default)]
struct JsonLdProducts<'a> {
    own: Vec<&'a Map<String, Value>>,
    nested: usize,
    listed: usize,
}

/// Collects the outermost product nodes of `value` under `context` and the lists the page holds
/// as its own into `found`. serde_json refuses a document nested deeper than its recursion limit,
/// so the walk is bounded by what parses.
fn walk_json_ld<'a>(
    value: &'a Value,
    context: &JsonLdContext,
    position: Position,
    found: &mut JsonLdProducts<'a>,
) {
    match value {
        Value::Array(members) => {
            for member in members {
                walk_json_ld(member, context, position, found);
            }
        }
        Value::Object(node) => {
            let context = context.of_node(node);
            let types = node
                .get("@type")
                .map(|types| context.schema_org_types(types))
                .unwrap_or_default();
            let is = |kinds: &[&str]| types.iter().any(|term| kinds.contains(&term.as_str()));
            if is(PRODUCT_TYPES) {
                match position {
                    Position::TopLevel | Position::PageContent => found.own.push(node),
                    Position::Nested => found.nested += 1,
                }
                return;
            }
            if position != Position::Nested && is(LIST_TYPES) {
                found.listed = found.listed.max(listed_items(node, &context));
            }
            let page = is(PAGE_TYPES);
            for (key, child) in node.iter().filter(|(key, _)| *key != "@context") {
                let child_position = match (position, key.as_str()) {
                    (Position::TopLevel, "@graph") => Position::TopLevel,
                    (Position::TopLevel, "mainEntity") if page => Position::PageContent,
                    _ => Position::Nested,
                };
                walk_json_ld(child, &context, child_position, found);
            }
        }
        _ => {}
    }
}

/// schema.org's lists of items a page holds as its own listing. `BreadcrumbList`, a subtype of
/// `ItemList`, is the page's place in the shop, not a listing, and is a type of its own here.
const LIST_TYPES: &[&str] = &["ItemList", "OfferCatalog"];

/// How many entries of a list node's `itemListElement` name an item: a URL, a node of a product
/// type, or a `ListItem` (or any other node) whose `item` or `url` names one, as a category page
/// lists its products.
fn listed_items(list: &Map<String, Value>, context: &JsonLdContext) -> usize {
    let names_item = |entry: &Value| match entry {
        Value::String(url) => !url.trim().is_empty(),
        Value::Object(entry) => {
            let types = entry
                .get("@type")
                .map(|types| context.of_node(entry).schema_org_types(types))
                .unwrap_or_default();
            types
                .iter()
                .any(|term| PRODUCT_TYPES.contains(&term.as_str()))
                || ["item", "url"].into_iter().any(|key| {
                    entry.get(key).is_some_and(|item| match item {
                        Value::String(url) => !url.trim().is_empty(),
                        Value::Object(_) => true,
                        _ => false,
                    })
                })
        }
        _ => false,
    };
    match list.get("itemListElement") {
        Some(Value::Array(entries)) => entries.iter().filter(|entry| names_item(entry)).count(),
        Some(entry) => usize::from(names_item(entry)),
        None => 0,
    }
}

/// The products of the page's microdata: an item whose `itemtype` names a product type, counted as
/// the page's own when no other item holds it except a web page item. One pass over the document
/// tracks each node's position, so a deeply nested page costs no more than its size.
fn microdata_products(document: &Html) -> ProductCount {
    let mut count = ProductCount::default();
    let mut pending = vec![(document.tree.root(), Position::TopLevel)];
    while let Some((node, position)) = pending.pop() {
        let item = ElementRef::wrap(node).filter(|element| element.attr("itemscope").is_some());
        let types: Vec<&str> = item
            .and_then(|item| item.attr("itemtype"))
            .map(|types| {
                types
                    .split_ascii_whitespace()
                    .filter_map(schema_org_term_of_iri)
                    .collect()
            })
            .unwrap_or_default();
        if types.iter().any(|term| PRODUCT_TYPES.contains(term)) {
            match position {
                Position::TopLevel | Position::PageContent => count.own += 1,
                Position::Nested => count.nested += 1,
            }
            continue;
        }
        let child_position = match item {
            Some(_) if types.iter().any(|term| PAGE_TYPES.contains(term)) => position,
            Some(_) => Position::Nested,
            None => position,
        };
        pending.extend(node.children().map(|child| (child, child_position)));
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(head: &str, body: &str) -> String {
        format!("<!doctype html><html><head>{head}</head><body>{body}</body></html>")
    }

    fn json_ld(scripts: &[&str]) -> String {
        let scripts: String = scripts
            .iter()
            .map(|json| format!(r#"<script type="application/ld+json">{json}</script>"#))
            .collect();
        page(&scripts, "<h1>Chair</h1>")
    }

    #[rstest::rstest]
    #[case::product(r#"{"@context": "https://schema.org", "@type": "Product", "name": "Chair"}"#)]
    #[case::product_group(r#"{"@context": "https://schema.org", "@type": "ProductGroup", "hasVariant": [{"@type": "Product", "name": "52"}, {"@type": "Product", "name": "54"}]}"#)]
    #[case::individual_product(r#"{"@type": "IndividualProduct", "name": "Lot 12"}"#)]
    #[case::product_model(r#"{"@type": "https://schema.org/ProductModel"}"#)]
    #[case::vehicle(r#"{"@type": "Car", "name": "1934 Bugatti"}"#)]
    #[case::in_graph(r#"{"@context": "https://schema.org", "@graph": [{"@type": "WebSite"}, {"@type": "ProductGroup", "name": "Ring"}]}"#)]
    #[case::main_entity_of_a_web_page(r#"{"@context": "https://schema.org", "@type": "ItemPage", "mainEntity": {"@type": "Product", "name": "Chair"}}"#)]
    #[case::with_related_products(r#"[{"@type": "Product", "url": "https://shop.example/chair"}, {"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "item": {"@type": "Product"}}, {"@type": "ListItem", "item": {"@type": "Product"}}]}]"#)]
    #[case::same_product_twice(r#"[{"@type": "Product", "url": "https://shop.example/chair"}, {"@type": "Product", "url": "https://shop.example/chair"}]"#)]
    fn should_find_one_product(#[case] json: &str) {
        assert_eq!(
            ProductMarkup::of_page(&json_ld(&[json])),
            ProductMarkup::OneProduct,
            "{json}"
        );
    }

    #[rstest::rstest]
    #[case::no_markup(page("", "<h1>Rings</h1>"))]
    #[case::other_types(json_ld(&[r#"{"@context": "https://schema.org", "@graph": [{"@type": "Organization"}, {"@type": "BreadcrumbList", "itemListElement": [{"@type": "ListItem", "item": "https://shop.example/rings/"}]}]}"#]))]
    #[case::breadcrumbs(json_ld(&[r#"{"@type": "BreadcrumbList", "itemListElement": [{"@type": "ListItem", "item": "https://shop.example/"}, {"@type": "ListItem", "item": "https://shop.example/rings/"}]}"#]))]
    #[case::list_of_one_item(json_ld(&[r#"{"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "url": "https://shop.example/a/"}]}"#]))]
    #[case::removal_notice(page("", "<h1>This item is no longer available</h1>"))]
    #[case::product_of_another_vocabulary(json_ld(&[r#"{"@context": "https://example.com/vocab", "@type": "Product"}"#]))]
    fn should_find_no_product(#[case] html: String) {
        assert_eq!(ProductMarkup::of_page(&html), ProductMarkup::NoProduct);
    }

    #[rstest::rstest]
    #[case::top_level_cards(r#"[{"@type": "Product", "url": "https://shop.example/a"}, {"@type": "Product", "url": "https://shop.example/b"}]"#)]
    #[case::item_list_of_products(r#"{"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "item": {"@type": "Product", "url": "https://shop.example/a"}}, {"@type": "ListItem", "item": {"@type": "ProductGroup", "url": "https://shop.example/b"}}]}"#)]
    #[case::collection_page_listing_products(r#"{"@type": "CollectionPage", "mainEntity": {"@type": "ItemList", "itemListElement": [{"@type": "Product"}, {"@type": "Product"}]}}"#)]
    #[case::item_list_of_product_urls(r#"{"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "url": "https://shop.example/a/"}, {"@type": "ListItem", "url": "https://shop.example/b/"}]}"#)]
    #[case::category_page_graph(r#"{"@context": "https://schema.org", "@graph": [{"@type": "Organization"}, {"@type": "BreadcrumbList", "itemListElement": [{"@type": "ListItem", "item": "https://shop.example/"}, {"@type": "ListItem", "item": "https://shop.example/rings/"}]}, {"@type": "ItemList", "itemListElement": [{"@type": "ListItem", "position": 1, "name": "Ring", "url": "https://shop.example/ring/"}, {"@type": "ListItem", "position": 2, "name": "Brooch", "url": "https://shop.example/brooch/"}]}]}"#)]
    #[case::offer_catalog_of_url_strings(r#"{"@type": "OfferCatalog", "itemListElement": ["https://shop.example/a/", "https://shop.example/b/"]}"#)]
    fn should_find_a_listing(#[case] json: &str) {
        assert_eq!(
            ProductMarkup::of_page(&json_ld(&[json])),
            ProductMarkup::Listing,
            "{json}"
        );
    }

    #[rstest::rstest]
    #[case::script_that_does_not_parse(json_ld(&[r#"{"@type": "WebSite"}"#, r#"{"@type": "Product", "name": "Chair",}"#]))]
    #[case::one_nested_product(json_ld(&[r#"{"@type": "Offer", "itemOffered": {"@type": "Product", "name": "Chair"}}"#]))]
    #[case::product_nested_in_a_non_page_item(json_ld(&[r#"{"@type": "Organization", "makesOffer": {"@type": "Offer", "itemOffered": {"@type": "ProductGroup"}}}"#]))]
    #[case::json_ld_and_microdata_disagree(page(
        r#"<script type="application/ld+json">{"@type": "Product", "url": "https://shop.example/a"}</script>"#,
        r#"<div itemscope itemtype="https://schema.org/Product"></div><div itemscope itemtype="https://schema.org/Product"></div>"#
    ))]
    fn should_find_the_markup_unclear(#[case] html: String) {
        assert_eq!(ProductMarkup::of_page(&html), ProductMarkup::Unclear);
    }

    #[rstest::rstest]
    #[case::product(r#"<div itemscope itemtype="https://schema.org/Product"><h1 itemprop="name">Chair</h1></div>"#)]
    #[case::product_group(r#"<div itemscope itemtype="http://schema.org/ProductGroup"></div>"#)]
    #[case::in_a_web_page_item(r#"<div itemscope itemtype="https://schema.org/WebPage"><div itemscope itemtype="https://schema.org/Product"></div></div>"#)]
    #[case::with_a_nested_offer_and_variant(r#"<div itemscope itemtype="https://schema.org/ProductGroup"><div itemprop="hasVariant" itemscope itemtype="https://schema.org/Product"></div><div itemprop="offers" itemscope itemtype="https://schema.org/Offer"></div></div>"#)]
    fn should_find_one_product_in_microdata(#[case] body: &str) {
        assert_eq!(
            ProductMarkup::of_page(&page("", body)),
            ProductMarkup::OneProduct,
            "{body}"
        );
    }

    #[test]
    fn should_find_a_microdata_grid_of_products_in_a_list() {
        let card = r#"<li itemprop="itemListElement" itemscope itemtype="https://schema.org/Product"></li>"#;
        let html = page(
            "",
            &format!(r#"<ul itemscope itemtype="https://schema.org/ItemList">{card}{card}</ul>"#),
        );
        assert_eq!(ProductMarkup::of_page(&html), ProductMarkup::Listing);
    }

    #[test]
    fn should_classify_only_listings() {
        assert!(ProductMarkup::Listing.shows_a_listing());
        assert!(!ProductMarkup::NoProduct.shows_a_listing());
        assert!(!ProductMarkup::OneProduct.shows_a_listing());
        assert!(!ProductMarkup::Unclear.shows_a_listing());
    }
}
