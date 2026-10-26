use listing_source_core::{Domain, InvalidDomain as NoDomainError};
use regex::Regex;
use spider::compact_str::CompactString;
use std::collections::{BTreeMap, HashSet};
use url_normalize::Options;

const BLACKLIST_URL_SUBSTRINGS: &[&str] = &[
    "cart",
    "wishlist",
    "?replytocom=",
    "&replytocom=",
    "/wp-admin/",
    ".jpg",
    ".jpeg",
    ".png",
    ".webp",
    ".gif",
    ".svg",
    ".ico",
    ".pdf",
    ".zip",
    ".csv",
    ".xml",
    ".json",
    "/download",
    "downloadproductimages",
    "/file",
    "/files",
    "/attachment",
    "/attachments",
];

/// A crawled page URL in the spelling the crawler stores: the URL as fetched, with only the
/// removals `docs/crawler/networking.md#stored-urls` documents.
/// The path and query keep their percent-encoding and parameter order byte for byte, so the
/// stored URL is the one robots.txt was checked against and the one the scraper requests.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrawledUrl(url::Url);

impl CrawledUrl {
    pub fn new(mut url: url::Url) -> Self {
        url.set_fragment(None);
        // Both only fail for URLs without a host, which carry no credentials.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        drop_trailing_slashes(&mut url);
        drop_tracking_parameters(&mut url);
        Self(url)
    }

    /// The spelling the crawler stored this URL under while it re-encoded queries (`%20` as
    /// `+`, sorted parameters, decoded path escapes), when that spelling differs from this one.
    /// Rows stored then are found under it until the spider rediscovers them.
    pub fn legacy_spelling(&self) -> Option<url::Url> {
        let legacy = legacy_spelling_of(&self.0);
        (legacy != self.0).then_some(legacy)
    }

    pub fn as_url(&self) -> &url::Url {
        &self.0
    }

    pub fn matches_pattern(&self, pattern: &Regex) -> bool {
        pattern.is_match(self.0.as_str())
    }

    pub fn classify(
        &self,
        pattern: Option<&Regex>,
    ) -> crate::spider::classification::url_metadata::UrlClass {
        if let Some(regex) = pattern
            && self.matches_pattern(regex)
        {
            return crate::spider::classification::url_metadata::UrlClass::ProductListing;
        }

        let lower = self.0.as_str().to_ascii_lowercase();

        if [
            "imprint",
            "impressum",
            "mentions-legales",
            "informazioni-legali",
            "aviso-legal",
            "legal-notice",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return crate::spider::classification::url_metadata::UrlClass::Imprint;
        }

        if [
            "category",
            "categories",
            "kategorie",
            "kategorier",
            "categorie",
            "categorias",
            "collections",
            "shop",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return crate::spider::classification::url_metadata::UrlClass::Category;
        }

        if [
            "about",
            "about-us",
            "contact",
            "faq",
            "terms",
            "privacy",
            "uber",
            "kontakt",
            "agb",
            "datenschutz",
            "chi-siamo",
            "contatti",
            "termini",
            "quienes-somos",
            "contacto",
            "politica-de-privacidad",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return crate::spider::classification::url_metadata::UrlClass::Info;
        }

        crate::spider::classification::url_metadata::UrlClass::Other
    }

    pub fn is_blacklisted(&self) -> bool {
        Self::is_blacklisted_url(self.0.as_str())
    }

    /// Applies the same rule as [`Self::blacklist_patterns`] to a URL that has not been
    /// normalized, as Spider does with the links it admits.
    pub fn is_blacklisted_url(url: &str) -> bool {
        BLACKLIST_URL_SUBSTRINGS
            .iter()
            .any(|pattern| url.contains(pattern))
    }

    pub fn blacklist_patterns() -> Vec<CompactString> {
        BLACKLIST_URL_SUBSTRINGS
            .iter()
            .map(|pattern| CompactString::from(regex::escape(pattern)))
            .collect()
    }
}

impl std::fmt::Display for CrawledUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for CrawledUrl {
    type Error = url::ParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let parsed = url::Url::parse(value)?;
        Ok(Self::new(parsed))
    }
}

/// A row stored under `legacy`, the earlier spelling of the fetched URL `fetched`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySpellingRename {
    pub legacy: url::Url,
    pub fetched: url::Url,
}

/// The renames that move rows stored under the earlier spelling to the URLs fetched in `fetched`.
/// A legacy spelling that is itself among the fetched URLs is a page of its own and is not
/// renamed; several fetched URLs sharing one legacy spelling take it in URL order, so the first
/// keeps the row and the others become rows of their own.
pub fn legacy_spelling_renames(fetched: &[url::Url]) -> Vec<LegacySpellingRename> {
    let fetched_spellings = fetched.iter().map(url::Url::as_str).collect::<HashSet<_>>();
    let mut renames = BTreeMap::<String, LegacySpellingRename>::new();
    for url in fetched {
        let Some(legacy) = CrawledUrl::new(url.clone()).legacy_spelling() else {
            continue;
        };
        if fetched_spellings.contains(legacy.as_str()) {
            continue;
        }
        renames
            .entry(legacy.to_string())
            .and_modify(|rename| {
                if url.as_str() < rename.fetched.as_str() {
                    rename.fetched = url.clone();
                }
            })
            .or_insert_with(|| LegacySpellingRename {
                legacy,
                fetched: url.clone(),
            });
    }
    renames.into_values().collect()
}

/// `/products/1/` and `/products/1` name one page; the root path keeps its slash.
fn drop_trailing_slashes(url: &mut url::Url) {
    let path = url.path();
    if path.len() <= 1 || !path.ends_with('/') {
        return;
    }
    let trimmed = path.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed }.to_owned();
    url.set_path(&trimmed);
}

/// Drops `utm_*` campaign parameters and keeps every other parameter exactly as fetched. A
/// query left empty is dropped with its `?`.
fn drop_tracking_parameters(url: &mut url::Url) {
    let Some(query) = url.query() else {
        return;
    };
    let kept = query
        .split('&')
        .filter(|pair| !is_tracking_parameter(pair))
        .collect::<Vec<_>>()
        .join("&");
    if kept.is_empty() {
        url.set_query(None);
    } else if kept != query {
        url.set_query(Some(&kept));
    }
}

fn is_tracking_parameter(pair: &str) -> bool {
    url::form_urlencoded::parse(pair.as_bytes())
        .next()
        .is_some_and(|(key, _)| {
            key.get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("utm_"))
        })
}

/// The normalization the crawler applied before it kept the fetched encoding, kept only to find
/// rows stored under it.
fn legacy_spelling_of(url: &url::Url) -> url::Url {
    let raw = url.as_str();
    let options = Options {
        strip_hash: true,
        remove_trailing_slash: true,
        remove_single_slash: false,
        strip_www: false,
        ..Default::default()
    };
    let normalized = match url_normalize::normalize_url(raw.trim(), &options) {
        Ok(normalized) => normalized,
        Err(_) => raw.trim().trim_end_matches('/').to_string(),
    };
    url::Url::parse(&normalized).unwrap_or_else(|_| url.clone())
}

pub fn extract_crawl_root_domain(crawl_root_url: &str) -> Result<Domain, NoDomainError> {
    let parsed = url::Url::parse(crawl_root_url).map_err(|_| NoDomainError::new(crawl_root_url))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| NoDomainError::new(crawl_root_url))?;
    Domain::try_from(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::RegexSet;

    fn normalize_for_test(input: &str) -> String {
        if let Ok(parsed) = url::Url::parse(input) {
            CrawledUrl::new(parsed).to_string()
        } else {
            input.to_string()
        }
    }

    #[test]
    fn should_normalize_url_when_fragment_exists_for_comparison() {
        let input = "https://www.example.com/product/123?ref=google#reviews";
        let normalized = normalize_for_test(input);
        assert_eq!(normalized, "https://www.example.com/product/123?ref=google");
    }

    #[test]
    fn should_trim_trailing_slash_when_path_is_not_root_for_comparison() {
        let input = "https://www.example.com/products/";
        let normalized = normalize_for_test(input);
        assert_eq!(normalized, "https://www.example.com/products");
    }

    #[test]
    fn should_trim_trailing_slash_with_query_params() {
        let input = "https://www.example.com/products/?q=1";
        let normalized = normalize_for_test(input);
        assert_eq!(normalized, "https://www.example.com/products?q=1");
    }

    #[test]
    fn should_keep_root_slash_when_path_is_root_for_comparison() {
        let input = "https://www.example.com/";
        let normalized = normalize_for_test(input);
        assert_eq!(normalized, "https://www.example.com/");
    }

    #[test]
    fn should_keep_query_encoding_as_fetched_when_value_holds_space_plus_or_encoded_plus() {
        for fetched in [
            "https://hickmet.com/collections/vendors?q=A%2C%20B",
            "https://hickmet.com/collections/vendors?q=A%2C+B",
            "https://hickmet.com/collections/vendors?q=A%2C%2BB",
            "https://hickmet.com/collections/vendors?q=A%2c%2bB",
            "https://example.com/search?q=caf%C3%A9&page=2",
        ] {
            assert_eq!(normalize_for_test(fetched), fetched);
        }
    }

    #[test]
    fn should_keep_parameter_order_and_empty_values_when_query_is_stored() {
        let fetched = "https://example.com/index.php?id_product=12&controller=product&rewrite=";

        assert_eq!(normalize_for_test(fetched), fetched);
    }

    #[test]
    fn should_keep_path_encoding_as_fetched_when_path_holds_escapes() {
        for fetched in [
            "https://example.com/products/caf%c3%a9",
            "https://example.com/products/100%25-wool",
            "https://example.com/products/%7Efoo",
            "https://example.com/a//b",
        ] {
            assert_eq!(normalize_for_test(fetched), fetched);
        }
    }

    #[test]
    fn should_drop_only_tracking_parameters_when_query_holds_utm_parameters() {
        assert_eq!(
            normalize_for_test(
                "https://example.com/products/1?utm_source=mail&q=A%20B&UTM_Campaign=x"
            ),
            "https://example.com/products/1?q=A%20B"
        );
        assert_eq!(
            normalize_for_test("https://example.com/products/1?utm_source=mail"),
            "https://example.com/products/1"
        );
        assert_eq!(
            normalize_for_test("https://example.com/products/1?"),
            "https://example.com/products/1"
        );
    }

    #[test]
    fn should_drop_credentials_when_url_carries_them() {
        assert_eq!(
            normalize_for_test("https://user:secret@example.com/products/1"),
            "https://example.com/products/1"
        );
    }

    #[test]
    fn should_keep_normalization_stable_when_stored_url_is_normalized_again() {
        for fetched in [
            "https://example.com/products/1/?q=A%20B#gallery",
            "https://example.com/products/1?utm_source=x&b=2&a=1",
            "https://example.com/products//",
        ] {
            let stored = normalize_for_test(fetched);
            assert_eq!(normalize_for_test(&stored), stored, "{fetched}");
        }
    }

    #[test]
    fn should_name_legacy_spelling_when_earlier_normalization_re_encoded_the_url() {
        for (fetched, legacy) in [
            (
                "https://hickmet.com/collections/vendors?q=A%2C%20B",
                "https://hickmet.com/collections/vendors?q=A%2C+B",
            ),
            (
                "https://example.com/index.php?id_product=12&controller=product",
                "https://example.com/index.php?controller=product&id_product=12",
            ),
            (
                "https://example.com/products/%7Efoo",
                "https://example.com/products/~foo",
            ),
        ] {
            let url = CrawledUrl::try_from(fetched).unwrap();

            assert_eq!(
                url.legacy_spelling().map(String::from).as_deref(),
                Some(legacy),
                "{fetched}"
            );
        }
    }

    #[test]
    fn should_name_no_legacy_spelling_when_earlier_normalization_kept_the_url() {
        for fetched in [
            "https://example.com/products/1",
            "https://hickmet.com/collections/vendors?q=A%2C+B",
            "https://hickmet.com/collections/vendors?q=A%2C%2BB",
            "https://example.com/products/1?a=1&b=2",
        ] {
            let url = CrawledUrl::try_from(fetched).unwrap();

            assert_eq!(url.legacy_spelling(), None, "{fetched}");
        }
    }

    fn urls(raw: &[&str]) -> Vec<url::Url> {
        raw.iter()
            .map(|raw| url::Url::parse(raw).unwrap())
            .collect()
    }

    #[test]
    fn should_rename_legacy_spelling_to_fetched_url_when_spider_rediscovers_it() {
        let renames = legacy_spelling_renames(&urls(&[
            "https://hickmet.com/collections/vendors?q=A%2C%20B",
            "https://hickmet.com/products/1",
        ]));

        assert_eq!(
            renames,
            vec![LegacySpellingRename {
                legacy: url::Url::parse("https://hickmet.com/collections/vendors?q=A%2C+B")
                    .unwrap(),
                fetched: url::Url::parse("https://hickmet.com/collections/vendors?q=A%2C%20B")
                    .unwrap(),
            }]
        );
    }

    #[test]
    fn should_keep_legacy_spelling_when_shop_links_it_as_well() {
        let renames = legacy_spelling_renames(&urls(&[
            "https://hickmet.com/collections/vendors?q=A%2C%20B",
            "https://hickmet.com/collections/vendors?q=A%2C+B",
        ]));

        assert!(renames.is_empty());
    }

    #[test]
    fn should_rename_legacy_spelling_to_first_fetched_url_when_several_share_it() {
        let renames = legacy_spelling_renames(&urls(&[
            "https://example.com/search?b=A%20B&a=1",
            "https://example.com/search?a=1&b=A%20B",
        ]));

        assert_eq!(renames.len(), 1);
        assert_eq!(
            renames[0].legacy.as_str(),
            "https://example.com/search?a=1&b=A+B"
        );
        assert_eq!(
            renames[0].fetched.as_str(),
            "https://example.com/search?a=1&b=A%20B"
        );
    }

    #[test]
    fn should_store_url_robots_txt_allowed_when_rule_distinguishes_space_from_plus() {
        // Shopify's default rules refuse literal and encoded `+` in collection URLs, not `%20`.
        let rules = crate::network::robots::RobotsRules::parse(
            "User-agent: *\nDisallow: /collections/*+*\nDisallow: /collections/*%2B*\n",
        );
        let space =
            CrawledUrl::try_from("https://hickmet.com/collections/vendors?q=A%2C%20B").unwrap();
        let plus =
            CrawledUrl::try_from("https://hickmet.com/collections/vendors?q=A%2C+B").unwrap();
        let encoded_plus =
            CrawledUrl::try_from("https://hickmet.com/collections/vendors?q=A%2C%2BB").unwrap();

        assert!(rules.allows(space.as_url()));
        assert!(!rules.allows(plus.as_url()));
        assert!(!rules.allows(encoded_plus.as_url()));
        let legacy = space
            .legacy_spelling()
            .expect("the earlier normalization re-encoded the space");
        assert!(!rules.allows(&legacy));
    }

    #[test]
    fn should_build_blacklist_patterns_matching_junk_urls_for_spider_blacklist() {
        let patterns = CrawledUrl::blacklist_patterns();
        let regex_set = RegexSet::new(patterns.iter().map(|pattern| pattern.as_str()))
            .expect("blacklist patterns should compile");

        assert!(regex_set.is_match("https://example.com/product/1?add-to-cart=123"));
        assert!(regex_set.is_match("https://example.com/product/1?a=1&replytocom=456"));
        assert!(regex_set.is_match("https://example.com/wp-admin/admin-ajax.php"));
        assert!(regex_set.is_match("https://example.com/product/downloadproductimages/581341"));
        assert!(regex_set.is_match("https://example.com/download/catalog.pdf"));
        assert!(regex_set.is_match("https://example.com/files/product-data.json"));
        assert!(regex_set.is_match("https://example.com/product/1/image.jpg"));
        assert!(!regex_set.is_match("https://example.com/product/1?a=1&b=2"));
    }

    #[test]
    fn should_return_true_when_blacklisted_query_parameter_exists_for_blacklist_check() {
        let url = CrawledUrl::new(
            url::Url::parse("https://example.com/product/1?add-to-cart=123").unwrap(),
        );

        assert!(url.is_blacklisted());
    }

    #[test]
    fn should_return_true_when_blacklisted_path_exists_for_blacklist_check() {
        let url = CrawledUrl::new(
            url::Url::parse("https://example.com/wp-admin/admin-ajax.php").unwrap(),
        );

        assert!(url.is_blacklisted());
    }

    #[test]
    fn should_return_true_when_download_or_file_path_exists_for_blacklist_check() {
        for raw_url in [
            "https://www.decorativecollective.com/product/downloadproductimages/581341",
            "https://example.com/download/catalog",
            "https://example.com/files/catalog",
            "https://example.com/attachment/123",
            "https://example.com/attachments/123",
        ] {
            let url = CrawledUrl::new(url::Url::parse(raw_url).unwrap());

            assert!(url.is_blacklisted(), "{raw_url} should be blacklisted");
        }
    }

    #[test]
    fn should_return_true_when_file_extension_exists_for_blacklist_check() {
        for raw_url in [
            "https://example.com/product/1.jpg",
            "https://example.com/product/1.jpeg",
            "https://example.com/product/1.png",
            "https://example.com/product/1.webp",
            "https://example.com/product/1.gif",
            "https://example.com/product/1.svg",
            "https://example.com/product/1.ico",
            "https://example.com/catalog.pdf",
            "https://example.com/archive.zip",
            "https://example.com/export.csv",
            "https://example.com/feed.xml",
            "https://example.com/product-data.json",
        ] {
            let url = CrawledUrl::new(url::Url::parse(raw_url).unwrap());

            assert!(url.is_blacklisted(), "{raw_url} should be blacklisted");
        }
    }

    #[test]
    fn should_return_false_when_url_not_blacklisted_for_blacklist_check() {
        let url =
            CrawledUrl::new(url::Url::parse("https://example.com/product/1?a=1&b=2").unwrap());

        assert!(!url.is_blacklisted());
    }

    #[test]
    fn should_classify_imprint_when_url_contains_supported_legal_keywords_for_url_classification() {
        let fr = CrawledUrl::new(url::Url::parse("https://example.com/mentions-legales").unwrap());
        let it =
            CrawledUrl::new(url::Url::parse("https://example.com/informazioni-legali").unwrap());
        let es = CrawledUrl::new(url::Url::parse("https://example.com/aviso-legal").unwrap());

        assert_eq!(
            fr.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Imprint
        );
        assert_eq!(
            it.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Imprint
        );
        assert_eq!(
            es.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Imprint
        );
    }

    #[test]
    fn should_classify_info_when_url_contains_supported_info_keywords_for_url_classification() {
        let de = CrawledUrl::new(url::Url::parse("https://example.com/datenschutz").unwrap());
        let it = CrawledUrl::new(url::Url::parse("https://example.com/chi-siamo").unwrap());
        let es =
            CrawledUrl::new(url::Url::parse("https://example.com/politica-de-privacidad").unwrap());

        assert_eq!(
            de.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Info
        );
        assert_eq!(
            it.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Info
        );
        assert_eq!(
            es.classify(None),
            crate::spider::classification::url_metadata::UrlClass::Info
        );
    }
}
