//! URLs the spider never follows or stores: shop actions, back office, downloads and files.
//!
//! The rules match whole path segments, whole tokens of a segment and parameter names, never
//! substrings, so a product slug such as `cartier-watch` or `cartel-clock` stays crawlable while
//! `/cart` does not.

use regex::RegexSet;
use spider::compact_str::CompactString;
use std::sync::LazyLock;

/// Path segments of actions and non-page resources, matched ASCII case-insensitively and with
/// or without a file extension (`/cart`, `/cart.php`, `/account/login/`). PrestaShop's
/// `controller` and OpenCart's `route` parameters name the same actions
/// (`?controller=my-account`, `?route=checkout/cart`).
const ACTION_SEGMENTS: &[&str] = &[
    // cart and checkout
    "cart",
    "add-to-cart",
    "basket",
    "warenkorb",
    "panier",
    "carrello",
    "carrito",
    "winkelwagen",
    "checkout",
    "kasse",
    // wishlists, including the misspelling Marc Maison serves
    "wishlist",
    "whishlist",
    "add-to-wishlist",
    "add-to-whishlist",
    "wunschliste",
    "wunschzettel",
    "merkzettel",
    "compare",
    // login, registration and accounts
    "login",
    "logout",
    "signin",
    "sign-in",
    "signup",
    "sign-up",
    "register",
    "registrieren",
    "anmelden",
    "connexion",
    "authentication",
    "customer_authentication",
    "password",
    "account",
    "my-account",
    "myaccount",
    "mein-konto",
    "mon-compte",
    // site search
    "search",
    "suche",
    "recherche",
    "ricerca",
    "buscar",
    "catalogsearch",
    // back office, downloads and file stores
    "wp-admin",
    "wp-login",
    "download",
    "downloads",
    "downloadproductimages",
    "file",
    "files",
    "attachment",
    "attachments",
    // printer-friendly copies of a page
    "print",
    "printable",
    "printer-friendly",
    "print-friendly",
];

/// Action words that also name the action as one token of a segment or file stem whose tokens
/// `-`, `_` or `.` separate (`/product-compare`, `/wishlist_add/12`,
/// `catalogue_detail_printerfriendly.php`). Only words no product slug uses as a word: `cart`,
/// `basket`, `register`, `search`, `file` and `print` name products too (`tea-cart`,
/// `wicker-basket`, `cash-register`, `woodblock-print`), so they match as whole segments only.
const ACTION_TOKENS: &[&str] = &[
    "wishlist",
    "whishlist",
    "wunschliste",
    "wunschzettel",
    "merkzettel",
    "compare",
    "login",
    "logout",
    "signin",
    "signup",
    "checkout",
    "warenkorb",
    "myaccount",
    "catalogsearch",
    "printerfriendly",
    "printer[-_.]friendly",
    "print[-_.]friendly",
];

/// Query parameters that trigger an action or a site search when the link is followed. `s` is
/// WordPress's search.
const ACTION_PARAMETERS: &[&str] = &[
    "s",
    "wc-ajax",
    "add-to-cart",
    "add_to_cart",
    "add-to-wishlist",
    "add_to_wishlist",
    "remove_item",
    "replytocom",
    "print",
];

/// Parameters whose value is a route of path-like segments, with the [`ACTION_SEGMENTS`] and
/// PrestaShop's checkout controllers as actions.
const ROUTE_PARAMETERS: &[&str] = &["route", "controller"];
const ROUTE_ONLY_ACTIONS: &[&str] = &["order", "order-opc"];

/// Extensions of files that are not HTML pages, matched on the last path segment.
const FILE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "svg", "ico", "pdf", "zip", "csv", "xml", "json",
];

/// Scheme and authority, so the rules below never look at the host.
const ORIGIN: &str = r"^[A-Za-z][A-Za-z0-9+.-]*://[^/?#]*";
/// The start of a query parameter, also behind an undecoded `&amp;`, `&#038;` or `&#x26;`, which
/// Spider judges before the link decoder decodes it. Spider reads the `#` of a numeric reference
/// as the start of a fragment, so the parameter may follow it.
pub(super) const PARAMETER_START: &str = r"^[^?#]*\?(?:[^#]*&(?:amp;|#0*38;|#[Xx]0*26;)?|amp;)?";

static BLACKLIST: LazyLock<RegexSet> =
    LazyLock::new(|| RegexSet::new(patterns()).expect("URL blacklist patterns compile"));

pub(super) fn is_blacklisted(url: &str) -> bool {
    BLACKLIST.is_match(url)
}

/// The blacklist as regexes over an absolute URL, in the form Spider's deny list takes.
pub(super) fn patterns() -> Vec<CompactString> {
    let segments = alternation(ACTION_SEGMENTS);
    vec![
        format!(r"{ORIGIN}/(?:[^?#]*/)?(?i:{segments})(?:\.[A-Za-z0-9]+)?(?:[/?#;]|$)").into(),
        format!(
            r"{ORIGIN}/(?:[^?#]*/)?(?:[^/?#;]*[-_.])?(?i:{})(?:[-_.][^/?#;]*)?(?:[/?#;]|$)",
            ACTION_TOKENS.join("|")
        )
        .into(),
        format!(
            r"{PARAMETER_START}(?i:{})(?:[=&#]|$)",
            alternation(ACTION_PARAMETERS)
        )
        .into(),
        format!(
            r"{PARAMETER_START}(?i:{})=(?:[^&#]*(?:/|%2[Ff]))?(?i:{segments}|{})(?:[/&#]|%2[Ff]|$)",
            alternation(ROUTE_PARAMETERS),
            alternation(ROUTE_ONLY_ACTIONS),
        )
        .into(),
        format!(
            r"{ORIGIN}/(?:[^?#]*/)?[^/?#]*\.(?i:{})(?:[?#;]|$)",
            alternation(FILE_EXTENSIONS)
        )
        .into(),
    ]
}

fn alternation(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| regex::escape(word))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_refuse_action_urls_seen_on_shops() {
        for action in [
            "https://japantik.com/customer_authentication/login?return_to=%2Fproducts%2Fnetsuke",
            "https://japantik.com/account/login",
            "https://www.marcmaison.com/en/whishlist/add/4512",
            "https://example.com/wishlist/",
            "https://example.com/cart",
            "https://example.com/Cart.php?action=add",
            "https://example.com/checkout/cart/",
            "https://example.com/my-account/lost-password/",
            "https://example.com/de/suche?q=stuhl",
            "https://example.com/search?q=chair&type=product",
            "https://example.com/catalogsearch/result/?q=lamp",
            "https://example.com/product/1?add-to-cart=123",
            "https://example.com/product/1?amp;add-to-cart=123",
            "https://example.com/post?p=1&amp;replytocom=55",
            "https://example.com/post?p=1&replytocom=55",
            "https://example.com/index.php?controller=my-account",
            "https://example.com/index.php?controller=cart&add=1",
            "https://example.com/index.php?route=checkout/cart",
            "https://example.com/index.php?route=account/wishlist/add",
            "https://example.com/index.php?route=checkout%2Fcart",
            "https://example.com/index.php?controller=order&step=1",
            "https://example.com/?s=stuhl&post_type=product",
            "https://example.com/?wc-ajax=add_to_cart",
            "https://example.com/shop/?product_cat=lamps&#038;add-to-cart=12",
            "https://example.com/en/add-to-whishlist/4512",
            "https://example.com/wp-admin/admin-ajax.php",
            "https://example.com/wp-login.php?action=lostpassword",
            "https://www.antique-silver.com/product-compare?item=4711",
            "https://example.com/wishlist_add/12",
            "https://example.com/customer-login.html",
            "https://example.com/en/quick-checkout/",
            "https://www.marlboroughantiques.com/catalogue_detail_printerfriendly.php?id=1234",
            "https://example.com/products/oak-chair/printer-friendly",
            "https://example.com/products/oak-chair/print",
            "https://example.com/print.php?id=12",
            "https://example.com/printable/12",
            "https://example.com/product_print_friendly.php?id=12",
            "https://example.com/products/oak-chair?print=1",
        ] {
            assert!(is_blacklisted(action), "{action} should be refused");
        }
    }

    #[test]
    fn should_refuse_downloads_and_files() {
        for file in [
            "https://www.decorativecollective.com/product/downloadproductimages/581341",
            "https://example.com/download/catalog",
            "https://example.com/downloads/catalog.pdf",
            "https://example.com/files/catalog",
            "https://example.com/attachment/123",
            "https://example.com/attachments/123",
            "https://example.com/product/1.jpg",
            "https://example.com/product/1.JPEG",
            "https://example.com/img/1.png?v=2",
            "https://example.com/product/1.webp",
            "https://example.com/product/1.gif",
            "https://example.com/logo.svg",
            "https://example.com/favicon.ico",
            "https://example.com/catalog.pdf",
            "https://example.com/archive.zip",
            "https://example.com/export.csv",
            "https://example.com/sitemap.xml",
            "https://example.com/products/chair.json",
        ] {
            assert!(is_blacklisted(file), "{file} should be refused");
        }
    }

    #[test]
    fn should_follow_pages_whose_words_only_contain_an_action() {
        for page in [
            "https://example.com/products/cartier-tank-watch",
            "https://example.com/brands/cartier",
            "https://example.com/product/cartel-clock-louis-xv",
            "https://example.com/shop/antique-carts/",
            "https://example.com/research/provenance",
            "https://example.com/accounting-ledger-1820",
            "https://example.com/profile-of-a-collector",
            "https://example.com/products/filet-crochet",
            "https://example.com/downloadable-prints",
            "https://example.com/product/1?a=1&b=2",
            "https://example.com/product/1?ref=cart",
            "https://example.com/index.php?controller=product&id_product=12",
            "https://example.com/index.php?route=product/product&product_id=50",
            "https://example.com/products/1?image=front.jpg",
            "https://example.com/json-collection",
            "https://cart.example.com/products/1",
            "https://example.com/products/1#cart",
            "https://example.com/products/1?sku=s",
            "https://example.com/order/how-to-order",
            "https://example.com/index.php?controller=product&id_product=12&order=x",
            "https://example.com/products/printed-cotton-quilt",
            "https://example.com/products/japanese-woodblock-print",
            "https://example.com/products/victorian-mahogany-tea-cart",
            "https://example.com/products/wicker-picnic-basket",
            "https://example.com/products/national-cash-register-1910",
            "https://example.com/products/blueprint-of-a-steam-engine",
            "https://example.com/products/comparison-of-hallmarks",
            "https://example.com/products/printers-tray-1900",
            "https://example.com/products/logo-plate?view=print-room",
        ] {
            assert!(!is_blacklisted(page), "{page} should be followed");
        }
    }
}
