//! Whether a product URL shows the same product as another product URL of its ListingSource that
//! carries the identity the URL's page derives
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`). A shop may reuse one identity
//! for different products, such as a stock code after a sale, so a shared identity alone never
//! makes two URLs one product: the crawler retires a URL as the duplicate of another only with
//! one of the signals below, and otherwise leaves both URLs as they are. The one-off migration
//! `20261117100002_retire_duplicate_identity_urls.sql` applies the signals SQL can express to the
//! stored URLs; `tests/fixtures/same_product.json` holds the cases both must answer alike.

use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::without_language_segment;
use regex::regex;
use url::Url;

/// Query parameters that name the language a page is shown in (`?Sprache=fr`, `?l=en`) or the
/// list page a product was linked from (`?page=2`). They do not tell two products apart, so two
/// URLs that differ only in them, on top of the parameters the crawler never stores
/// (`docs/crawler/networking.md#stored-urls`), name one product. They are not dropped when a URL
/// is stored, since a shop may keep its language versions apart by them.
const PRODUCT_NOISE_PARAMETERS: &[&str] = &[
    "lang", "language", "l", "lng", "hl", "locale", "sprache", "langue", "lingua", "idioma",
    "taal", "page", "pg",
];

/// A product URL as the same-product rule compares it.
#[derive(Debug, Clone, Copy)]
pub struct ProductUrlIdentity<'a> {
    pub url: &'a Url,
    /// The identity the URL's listing was captured under, `None` before its first capture.
    pub captured_identity: Option<&'a str>,
    /// The title the URL's last capture recorded, or the scraped one before its first capture.
    pub title: Option<&'a str>,
}

/// The signal that shows two URLs carrying one identity show one product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameProductEvidence {
    /// The URLs name the same page, apart from a trailing slash, doubled slashes, a language
    /// segment before the path and query parameters that do not identify a product.
    Spelling,
    /// The page identity is a whole path segment or query value of the URL's captured identity,
    /// which a rule read from the page URL (`/ukiyoe_detail.php?print_id=40475` and `40475`).
    IdentityToken,
    /// The captured titles are equal, or one of them is missing, and neither URL is a list page
    /// ([`is_list_page`]); equal titles count only when they name something
    /// ([`title_names_something`]).
    Title,
}

impl SameProductEvidence {
    /// The name the log events give the signal.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spelling => "spelling",
            Self::IdentityToken => "identity_token",
            Self::Title => "title",
        }
    }
}

/// The signal that shows `page`, whose page derives `page_identity`, and `carrier`, which carries
/// that identity, show one product; `None` when no signal does, and the shared identity alone
/// proves nothing. Spelling is checked first, then the identity token, then the titles.
pub fn same_product_evidence(
    page: ProductUrlIdentity<'_>,
    page_identity: &str,
    carrier: ProductUrlIdentity<'_>,
) -> Option<SameProductEvidence> {
    if product_spelling(page.url) == product_spelling(carrier.url) {
        Some(SameProductEvidence::Spelling)
    } else if page
        .captured_identity
        .is_some_and(|captured| is_identity_token_of(page_identity, captured))
    {
        Some(SameProductEvidence::IdentityToken)
    } else if titles_agree(page.title, carrier.title)
        && !is_list_page(page.url)
        && !is_list_page(carrier.url)
    {
        Some(SameProductEvidence::Title)
    } else {
        None
    }
}

/// `url` in the spelling two URLs of one product share: the [stored spelling](CrawledUrl)
/// without a trailing slash, doubled slashes in its path collapsed, without a language segment
/// before its path and without [`PRODUCT_NOISE_PARAMETERS`].
fn product_spelling(url: &Url) -> Url {
    let mut collapsed = CrawledUrl::new(url.clone()).as_url().clone();
    let path = collapse_slashes(collapsed.path());
    collapsed.set_path(&path);
    let crawled = CrawledUrl::new(collapsed);
    let mut spelling =
        without_language_segment(&crawled).unwrap_or_else(|| crawled.page_spelling());
    let kept = spelling.query().map(|query| {
        query
            .split('&')
            .filter(|pair| !is_noise_parameter(pair))
            .collect::<Vec<_>>()
            .join("&")
    });
    spelling.set_query(kept.as_deref().filter(|query| !query.is_empty()));
    spelling
}

fn collapse_slashes(path: &str) -> String {
    let mut collapsed = String::with_capacity(path.len());
    for character in path.chars() {
        if !(character == '/' && collapsed.ends_with('/')) {
            collapsed.push(character);
        }
    }
    collapsed
}

fn is_noise_parameter(pair: &str) -> bool {
    let name = pair.split_once('=').map_or(pair, |(name, _)| name);
    PRODUCT_NOISE_PARAMETERS
        .iter()
        .any(|noise| name.eq_ignore_ascii_case(noise))
}

/// Whether `identity` is a whole path segment or a whole query value of `captured`, a captured
/// identity a rule read from a link or path (one that starts with `/`, `http://` or `https://`).
/// Only whole tokens count: `5` is no token of `?id=15`.
fn is_identity_token_of(identity: &str, captured: &str) -> bool {
    let link = regex!(r"(?i)^(?:https?://[^/?#]*)?(/[^?#]*)?(?:\?([^#]*))?");
    if identity.is_empty()
        || identity == captured
        || !regex!(r"(?i)^(?:/|https?://)").is_match(captured)
    {
        return false;
    }
    let Some(parts) = link.captures(captured) else {
        return false;
    };
    let in_path = parts
        .get(1)
        .is_some_and(|path| path.as_str().split('/').any(|segment| segment == identity));
    let in_query = parts.get(2).is_some_and(|query| {
        query
            .as_str()
            .split('&')
            .filter_map(|pair| pair.split_once('=').map(|(_, value)| value))
            .any(|value| value == identity)
    });
    in_path || in_query
}

/// Whether two captured titles agree: they are equal and name something, or one is missing.
fn titles_agree(title: Option<&str>, other: Option<&str>) -> bool {
    match (title, other) {
        (Some(title), Some(other)) => title == other && title_names_something(title),
        _ => true,
    }
}

/// Whether a title has a character other than an ASCII digit, ASCII punctuation or ASCII
/// whitespace. A bare year or number (Gillmark's maps are titled `1893`) is shared by many
/// products of a shop, so two such titles being equal shows nothing.
fn title_names_something(title: &str) -> bool {
    title.chars().any(|character| {
        !(character.is_ascii_digit()
            || character.is_ascii_punctuation()
            || character == ' '
            || ('\t'..='\r').contains(&character))
    })
}

/// Whether `url`'s path is that of a numbered list page (`/page-5/`, `/page/5`, `/seite-2`).
/// Such a page shows one of its listed products first, so a schema derives that product's
/// identity and title from it; its title says nothing about it being that product.
fn is_list_page(url: &Url) -> bool {
    regex!(r"(?i)/(?:page|seite|pagina)(?:[-_]?|/)[0-9]+(?:/|$)").is_match(url.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity<'a>(
        url: &'a Url,
        captured: Option<&'a str>,
        title: Option<&'a str>,
    ) -> ProductUrlIdentity<'a> {
        ProductUrlIdentity {
            url,
            captured_identity: captured,
            title,
        }
    }

    fn evidence(
        url: &str,
        captured: Option<&str>,
        title: Option<&str>,
        page_identity: &str,
        carrier_url: &str,
        carrier_title: Option<&str>,
    ) -> Option<SameProductEvidence> {
        let url = Url::parse(url).unwrap();
        let carrier_url = Url::parse(carrier_url).unwrap();
        same_product_evidence(
            identity(&url, captured, title),
            page_identity,
            identity(&carrier_url, Some(page_identity), carrier_title),
        )
    }

    #[rstest::rstest]
    #[case::trailing_slash(
        "https://shop.example/products/vase/",
        "https://shop.example/products/vase"
    )]
    #[case::doubled_slashes(
        "https://shop.example/products//vase",
        "https://shop.example/products/vase/"
    )]
    #[case::tracking(
        "https://shop.example/products/vase?utm_source=mail&fbclid=1",
        "https://shop.example/products/vase"
    )]
    #[case::session_and_sort(
        "https://shop.example/item.php?id=7&osCsid=a1&sort=price",
        "https://shop.example/item.php?id=7"
    )]
    #[case::language_parameter(
        "https://www.wieland.example/artikel.php?id=5&Sprache=fr",
        "https://www.wieland.example/artikel.php?id=5&Sprache=de"
    )]
    #[case::short_language_parameter(
        "https://w-k.example/werk/12?l=en",
        "https://w-k.example/werk/12"
    )]
    #[case::language_segment(
        "https://hart.example/nl/products/vase",
        "https://hart.example/en/products/vase/"
    )]
    #[case::pager(
        "https://shop.example/view?p_idx=7&page=2",
        "https://shop.example/view?p_idx=7"
    )]
    fn should_take_two_spellings_of_one_page_for_one_product(
        #[case] url: &str,
        #[case] carrier: &str,
    ) {
        assert_eq!(
            evidence(url, Some("A"), Some("Vase"), "B", carrier, Some("Vaas")),
            Some(SameProductEvidence::Spelling)
        );
    }

    #[rstest::rstest]
    #[case::other_path(
        "https://shop.example/products/vase",
        "https://shop.example/products/dish"
    )]
    #[case::other_query_value(
        "https://shop.example/item.php?id=7",
        "https://shop.example/item.php?id=8"
    )]
    #[case::other_host(
        "https://shop.example/products/vase",
        "https://other.example/products/vase"
    )]
    fn should_not_take_other_pages_for_one_spelling(#[case] url: &str, #[case] carrier: &str) {
        assert_eq!(
            evidence(url, Some("A"), Some("Vase"), "B", carrier, Some("Dish")),
            None
        );
    }

    /// Hara Shobo captured its prints under the page path and now derives the print number.
    #[rstest::rstest]
    #[case::query_value("/ukiyoe/ukiyoe_detail.php?print_id=40475", "40475")]
    #[case::absolute_url(
        "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=40475&s=1",
        "40475"
    )]
    #[case::path_segment("/prints/40475/hiroshige", "40475")]
    fn should_take_a_page_identity_read_from_the_captured_link_for_one_product(
        #[case] captured: &str,
        #[case] page_identity: &str,
    ) {
        assert_eq!(
            evidence(
                "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=40475",
                Some(captured),
                Some("Hiroshige, Tokaido"),
                page_identity,
                "https://hara.example/print/40475",
                Some("Hiroshige: Tokaido station"),
            ),
            Some(SameProductEvidence::IdentityToken)
        );
    }

    #[rstest::rstest]
    #[case::part_of_a_value("/ukiyoe/ukiyoe_detail.php?print_id=15", "5")]
    #[case::part_of_a_segment("/prints/40475-hiroshige", "40475")]
    #[case::code_not_a_link("C327/5", "5")]
    #[case::query_name_not_value("/detail.php?40475=1", "40475")]
    fn should_take_only_whole_tokens_of_a_captured_link(
        #[case] captured: &str,
        #[case] page_identity: &str,
    ) {
        assert_eq!(
            evidence(
                "https://hara.example/a",
                Some(captured),
                Some("Hiroshige"),
                page_identity,
                "https://hara.example/b",
                Some("Hokusai"),
            ),
            None
        );
    }

    #[rstest::rstest]
    #[case::equal(Some("June 1971"), Some("June 1971"))]
    #[case::page_untitled(None, Some("June 1971"))]
    #[case::carrier_untitled(Some("June 1971"), None)]
    fn should_take_agreeing_or_missing_titles_for_one_product(
        #[case] title: Option<&str>,
        #[case] carrier_title: Option<&str>,
    ) {
        assert_eq!(
            evidence(
                "https://shop.example/product/3051/oak-chest-1/",
                Some("#3051"),
                title,
                "#3051",
                "https://shop.example/product/3051/oak-chest/",
                carrier_title,
            ),
            Some(SameProductEvidence::Title)
        );
    }

    /// Gillmark captured numbered list pages under the identity of the product each listed
    /// first, and titles its maps with bare years: neither a list page nor an equal year shows
    /// that two URLs are one product, in either direction.
    #[rstest::rstest]
    #[case::list_page_carrier_with_equal_years(
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("1893"),
        "https://www.gillmark.example/map/asia-210/page-5/",
        Some("1893")
    )]
    #[case::list_page_retiree_with_equal_years(
        "https://www.gillmark.example/map/asia-210/page-5/",
        Some("1893"),
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("1893")
    )]
    #[case::list_page_carrier_with_equal_titles(
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("European Russia, Johnston"),
        "https://www.gillmark.example/map/asia-210/page-5/",
        Some("European Russia, Johnston")
    )]
    #[case::list_page_carrier_untitled(
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("1893"),
        "https://www.gillmark.example/map/asia-210/page-5/",
        None
    )]
    #[case::list_page_retiree_untitled(
        "https://www.gillmark.example/map/asia-210/page/5",
        None,
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("1893")
    )]
    #[case::equal_years_on_two_product_pages(
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/",
        Some("1893"),
        "https://www.gillmark.example/map/asia-210/siberia-stieler-9301/",
        Some("1893")
    )]
    #[case::equal_numbers_and_punctuation(
        "https://shop.example/products/lot-17",
        Some("1882 / 3 (2)"),
        "https://shop.example/products/lot-18",
        Some("1882 / 3 (2)")
    )]
    fn should_find_no_title_evidence_on_a_list_page_or_in_a_bare_year(
        #[case] url: &str,
        #[case] title: Option<&str>,
        #[case] carrier_url: &str,
        #[case] carrier_title: Option<&str>,
    ) {
        assert_eq!(
            evidence(url, Some("9282"), title, "9282", carrier_url, carrier_title),
            None
        );
    }

    /// The list page guard and the bare-year guard hold back the title only: a spelling of the
    /// same page, such as Gillmark's English version, is still one product.
    #[rstest::rstest]
    #[case::language_parameter(
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/?l=en",
        "https://www.gillmark.example/map/asia-210/european-russia-johnston-9282/"
    )]
    #[case::language_segment(
        "https://www.gillmark.example/de/map/asia-210/european-russia-johnston-9282/",
        "https://www.gillmark.example/en/map/asia-210/european-russia-johnston-9282"
    )]
    fn should_still_take_a_translation_with_a_bare_year_title_for_one_product(
        #[case] url: &str,
        #[case] carrier_url: &str,
    ) {
        assert_eq!(
            evidence(
                url,
                Some("9282"),
                Some("1893"),
                "9282",
                carrier_url,
                Some("1893")
            ),
            Some(SameProductEvidence::Spelling)
        );
    }

    #[rstest::rstest]
    #[case::hyphen("/map/asia-210/page-5/")]
    #[case::underscore("/catalogue/page_12")]
    #[case::joined("/books/page3")]
    #[case::segment("/category/prints/page/2/")]
    #[case::german("/kategorie/grafik/seite-4/")]
    #[case::upper_case("/Shop/Page-7")]
    fn should_take_a_numbered_page_for_a_list_page(#[case] path: &str) {
        assert!(is_list_page(
            &Url::parse("https://shop.example")
                .unwrap()
                .join(path)
                .unwrap()
        ));
    }

    #[rstest::rstest]
    #[case::product_slug("/map/asia-210/european-russia-johnston-9282/")]
    #[case::number_inside_slug("/products/page-5-of-the-book-of-hours")]
    #[case::word_ending_in_page("/homepage-5/")]
    #[case::pager_parameter("/view?p_idx=7&page=2")]
    fn should_not_take_a_product_page_for_a_list_page(#[case] path: &str) {
        assert!(!is_list_page(
            &Url::parse("https://shop.example")
                .unwrap()
                .join(path)
                .unwrap()
        ));
    }

    #[rstest::rstest]
    #[case::words("June 1971", true)]
    #[case::code("C327", true)]
    #[case::non_ascii_letters("浮世絵 1850", true)]
    #[case::year("1893", false)]
    #[case::numbers_and_punctuation("No. 1882 / 3", true)]
    #[case::digits_and_punctuation("1882 / 3 (2)", false)]
    fn should_tell_whether_a_title_names_something(#[case] title: &str, #[case] names: bool) {
        assert_eq!(title_names_something(title), names);
    }

    /// Gillmark's page 14 was captured as May 1971 and now derives June 1971's identity; HaresFur
    /// reused the stock code `C327` for another piece, also when both pieces are titled by their
    /// year alone: neither is the carrier's product.
    #[rstest::rstest]
    #[case::gillmark(
        "https://www.gillmark.example/book/hertfordshire-countryside-238/page-14/",
        "5168",
        "May 1971",
        "5169",
        "https://www.gillmark.example/book/hertfordshire-countryside-238/june-1971-5169/",
        "June 1971"
    )]
    #[case::reused_stock_code(
        "https://haresfur.example/products/stig-lindberg-faience-dish",
        "C327",
        "Stig Lindberg Faience dish",
        "C327",
        "https://haresfur.example/products/rolf-palm-stoneware-vase",
        "Rolf Palm stoneware vase"
    )]
    #[case::reused_stock_code_with_bare_number_titles(
        "https://haresfur.example/products/c327-2",
        "C327",
        "1962",
        "C327",
        "https://haresfur.example/products/c327",
        "1962"
    )]
    fn should_find_no_evidence_for_another_product_under_a_shared_identity(
        #[case] url: &str,
        #[case] captured: &str,
        #[case] title: &str,
        #[case] page_identity: &str,
        #[case] carrier_url: &str,
        #[case] carrier_title: &str,
    ) {
        assert_eq!(
            evidence(
                url,
                Some(captured),
                Some(title),
                page_identity,
                carrier_url,
                Some(carrier_title)
            ),
            None
        );
    }

    /// The rows `tests/fixtures/same_product.json` holds, as migration 20261117100002 reads them.
    #[derive(serde::Deserialize)]
    struct FixtureRow {
        path: String,
        captured: String,
        page_identity: String,
        title: Option<String>,
        expected: String,
        evidence: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct FixtureGroup {
        name: String,
        rows: Vec<FixtureRow>,
    }

    #[derive(serde::Deserialize)]
    struct Fixtures {
        groups: Vec<FixtureGroup>,
    }

    /// The cases the migration test runs through the SQL: the rule answers each row as the
    /// fixture expects, so the runtime and the migration agree on them.
    #[test]
    fn should_answer_the_shared_fixtures_as_the_migration_does() {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../../tests/fixtures/same_product.json")).unwrap();
        let root = Url::parse("https://www.shop.example.com/").unwrap();
        let mut checked = 0;
        for group in &fixtures.groups {
            for row in group.rows.iter().filter(|row| row.expected != "survivor") {
                let carrier = group
                    .rows
                    .iter()
                    .find(|carrier| {
                        carrier.expected == "survivor" && carrier.captured == row.page_identity
                    })
                    .unwrap_or_else(|| {
                        panic!("{}: no survivor carries {}", group.name, row.page_identity)
                    });
                let url = root.join(&row.path).unwrap();
                let carrier_url = root.join(&carrier.path).unwrap();

                let found = same_product_evidence(
                    identity(&url, Some(&row.captured), row.title.as_deref()),
                    &row.page_identity,
                    identity(
                        &carrier_url,
                        Some(&carrier.captured),
                        carrier.title.as_deref(),
                    ),
                );

                assert_eq!(
                    found.map(SameProductEvidence::as_str),
                    row.evidence.as_deref(),
                    "{}: {}",
                    group.name,
                    row.path
                );
                assert_eq!(
                    found.is_some(),
                    row.expected == "retire",
                    "{}: {}",
                    group.name,
                    row.path
                );
                checked += 1;
            }
        }
        assert!(checked >= 10);
    }

    #[test]
    fn should_name_each_signal_for_the_log() {
        assert_eq!(SameProductEvidence::Spelling.as_str(), "spelling");
        assert_eq!(
            SameProductEvidence::IdentityToken.as_str(),
            "identity_token"
        );
        assert_eq!(SameProductEvidence::Title.as_str(), "title");
    }
}
