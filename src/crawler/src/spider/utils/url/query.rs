//! Query parameters that do not identify a page (`docs/crawler/networking.md#stored-urls`).
//!
//! Each family is a closed list of names a shop uses for the same page in another form. Every
//! other parameter is kept, because an unknown parameter may be the one that names the product
//! (`single.asp?id=113`, `index.php?id_product=12`), and dropping it would collapse a shop's
//! products onto one URL.

use spider::compact_str::CompactString;

/// A parameter name, matched ASCII case-insensitively on its decoded form.
#[derive(Clone, Copy)]
enum ParameterName {
    Exact(&'static str),
    Prefix(&'static str),
}

use ParameterName::{Exact, Prefix};

/// Campaign, click and session identifiers, and product variants: they mark who followed a link
/// or preselect an option on the product page, and a shop may link a page only with them
/// (Shopify collections link every product with `?variant=`). Such a link is still followed,
/// under its URL without them.
const FOLLOWED_PARAMETERS: &[ParameterName] = &[
    Prefix("utm_"),
    Exact("fbclid"),
    Exact("gclid"),
    Exact("gbraid"),
    Exact("wbraid"),
    Exact("dclid"),
    Exact("msclkid"),
    Exact("yclid"),
    Exact("igshid"),
    Exact("srsltid"),
    Exact("mc_cid"),
    Exact("mc_eid"),
    Exact("_ga"),
    Exact("_gl"),
    // osCommerce, xt:Commerce and Zen Cart put the session in every link without a cookie.
    Exact("oscsid"),
    Exact("xtcsid"),
    Exact("zenid"),
    Exact("phpsessid"),
    Exact("jsessionid"),
    Exact("sessionid"),
    Exact("variant"),
];

/// Facets, sort orders, grid or list views and currencies: the page the shop also links without
/// them, shown in another form. Spider does not follow links carrying
/// them, so facet combinations do not spend the crawl budget.
///
/// `view` is not among them: Joomla and VirtueMart route pages through it
/// (`option=com_virtuemart&view=productdetails`). Neither are page sizes (`limit`), since offset
/// pagination (`limitstart`) counts in them, nor names as generic as `filter` or `dir`, which
/// older shops use to name a page (`gallery.php?dir=chairs`).
const PRESENTATION_PARAMETERS: &[ParameterName] = &[
    // facets: WooCommerce layered navigation, Shopify storefront filters, generic forms
    Prefix("filter_"),
    Prefix("filter."),
    Prefix("filter["),
    Prefix("query_type_"),
    Exact("min_price"),
    Exact("max_price"),
    // sort orders: WooCommerce, Magento, PrestaShop, Shopify
    Exact("sort"),
    Exact("sort_by"),
    Exact("sortby"),
    Exact("orderby"),
    Exact("order_by"),
    Exact("order"),
    Exact("product_list_order"),
    Exact("product_list_dir"),
    // grid or list views
    Exact("view_mode"),
    Exact("viewmode"),
    Exact("product_list_mode"),
    Exact("gridcookie"),
    // currencies
    Exact("currency"),
    Exact("currency_code"),
    Exact("cur"),
];

impl ParameterName {
    fn matches(self, name: &str) -> bool {
        match self {
            Exact(exact) => name.eq_ignore_ascii_case(exact),
            Prefix(prefix) => name
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix)),
        }
    }

    /// The name as a regex over the raw query text, where `[` may be percent-encoded.
    fn pattern(self) -> String {
        let (name, rest) = match self {
            Exact(exact) => (exact, ""),
            Prefix(prefix) => (prefix, "[^=&#]*"),
        };
        let name = regex::escape(name).replace(r"\[", r"(?:\[|%5[Bb])");
        format!("{name}{rest}")
    }
}

/// Drops the parameters that do not identify the page and keeps every other parameter exactly
/// as fetched, in its order and encoding. A query left empty is dropped with its `?`.
pub(super) fn drop_non_identity_parameters(url: &mut url::Url) {
    let Some(query) = url.query() else {
        return;
    };
    let kept = query
        .split('&')
        .filter(|pair| !is_non_identity_parameter(pair))
        .collect::<Vec<_>>()
        .join("&");
    if kept.is_empty() {
        url.set_query(None);
    } else if kept != query {
        url.set_query(Some(&kept));
    }
}

fn is_non_identity_parameter(pair: &str) -> bool {
    url::form_urlencoded::parse(pair.as_bytes())
        .next()
        .is_some_and(|(name, _)| {
            FOLLOWED_PARAMETERS
                .iter()
                .chain(PRESENTATION_PARAMETERS)
                .any(|parameter| parameter.matches(&name))
        })
}

/// A Spider deny-list pattern that refuses links carrying a presentation parameter, also behind
/// an undecoded character reference, which Spider judges before the link decoder decodes it.
pub(super) fn presentation_link_pattern() -> CompactString {
    let names = PRESENTATION_PARAMETERS
        .iter()
        .map(|parameter| parameter.pattern())
        .collect::<Vec<_>>()
        .join("|");
    format!(
        r"{}(?i:{names})(?:[=&#]|$)",
        super::blacklist::PARAMETER_START
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn stripped(raw: &str) -> String {
        let mut url = url::Url::parse(raw).unwrap();
        drop_non_identity_parameters(&mut url);
        url.into()
    }

    /// The shops of the 2026-10-15 soak test (#2114).
    #[test]
    fn should_drop_facet_sort_view_currency_and_variant_parameters_seen_on_shops() {
        for (fetched, stored) in [
            (
                "https://antiquitaeten-tuebingen.de/shop/?filter_material=eiche&filter_stil-epoche=biedermeier",
                "https://antiquitaeten-tuebingen.de/shop/",
            ),
            (
                "https://antiquitaeten-tuebingen.de/shop/page/2/?gridcookie=list&query_type_material=or",
                "https://antiquitaeten-tuebingen.de/shop/page/2/",
            ),
            (
                "https://antikmoebel.de/moebel?currency=EUR&page=8",
                "https://antikmoebel.de/moebel?page=8",
            ),
            (
                "https://japantik.com/products/netsuke-hare?variant=4211",
                "https://japantik.com/products/netsuke-hare",
            ),
            (
                "https://carouche.fr/collections/all?filter.v.price.gte=100&sort_by=price-ascending&page=2",
                "https://carouche.fr/collections/all?page=2",
            ),
            (
                "https://example.com/c?filter%5Bcolor%5D=red&Product_List_Order=price&q=lamp",
                "https://example.com/c?q=lamp",
            ),
            (
                "https://example.com/catalog/product_info.php?products_id=7&osCsid=a1b2",
                "https://example.com/catalog/product_info.php?products_id=7",
            ),
            (
                "https://example.com/products/1?fbclid=x&utm_source=mail",
                "https://example.com/products/1",
            ),
        ] {
            assert_eq!(stripped(fetched), stored, "{fetched}");
        }
    }

    #[test]
    fn should_store_cur_currency_variants_as_the_bare_item_url() {
        for variant in ["?cur=EUR", "?cur=GBP", "?CUR=eur"] {
            assert_eq!(
                stripped(&format!("https://example.com/item/42{variant}")),
                "https://example.com/item/42",
                "{variant}"
            );
        }
    }

    #[test]
    fn should_keep_parameters_that_identify_or_paginate_a_page() {
        for fetched in [
            "https://antiquesbydesign.co.uk/single.asp?c=&offset=20&id=113",
            "https://alysantiques.com/index.php?category=26245&page=3",
            "https://example.com/index.php?id_product=12&controller=product&rewrite=",
            "https://example.com/index.php?option=com_virtuemart&view=productdetails&virtuemart_product_id=5",
            "https://example.com/category?limit=60&limitstart=120",
            "https://hickmet.com/collections/vendors?q=A%2C%20B",
            "https://example.com/?p=123",
            "https://example.com/c?filters=1&sorted=x&currency_switcher=1",
            "https://example.com/gallery.php?dir=chairs&filter=clocks",
        ] {
            assert_eq!(stripped(fetched), fetched);
        }
    }

    #[test]
    fn should_refuse_links_carrying_presentation_parameter_and_follow_others() {
        let pattern = Regex::new(&presentation_link_pattern()).unwrap();

        for refused in [
            "https://antikmoebel.de/moebel?currency=EUR&page=8",
            "https://antikmoebel.de/moebel?amp;currency=EUR&page=8",
            "https://alysantiques.com/c?page=3&amp;Sort=price",
            "https://example.com/shop/?filter_material=eiche",
            "https://example.com/c?filter%5Bcolor%5D=red",
            "https://example.com/c?filter.v.availability=1",
            "https://example.com/c?a=1&orderby",
            "https://example.com/shop/?product_cat=lamps&#038;orderby=price",
            "https://antikmoebel.de/moebel?page=8&#38;currency=EUR",
            "https://antikmoebel.de/moebel?page=8&#x26;currency=EUR",
        ] {
            assert!(pattern.is_match(refused), "{refused}");
        }
        for followed in [
            "https://example.com/currency/eur",
            "https://example.com/products/1?utm_source=mail",
            "https://japantik.com/products/netsuke-hare?variant=4211",
            "https://example.com/c?page=3&sorted=x",
            "https://example.com/c?q=currency=EUR",
            "https://example.com/c?a=variant",
            "https://example.com/c?id=1#sort=price",
        ] {
            assert!(!pattern.is_match(followed), "{followed}");
        }
    }
}
