use regex::Regex;
use shop_core::domain::{Domain, NoDomainError};
use spider::compact_str::CompactString;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrawledUrl(url::Url);

impl CrawledUrl {
    pub fn new(url: url::Url) -> Self {
        let raw = url.as_str();

        let options = Options {
            strip_hash: true,
            remove_trailing_slash: true,
            remove_single_slash: false,
            strip_www: false,
            ..Default::default()
        };

        let normalized = match url_normalize::normalize_url(raw.trim(), &options) {
            Ok(n) => n,
            Err(_) => raw.trim().trim_end_matches('/').to_string(),
        };

        if let Ok(parsed) = url::Url::parse(&normalized) {
            Self(parsed)
        } else {
            Self(url)
        }
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
        let raw = self.0.as_str();
        BLACKLIST_URL_SUBSTRINGS
            .iter()
            .any(|pattern| raw.contains(pattern))
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

pub fn extract_shop_base_url(shop_url: &str) -> Result<Domain, NoDomainError> {
    Domain::try_from(shop_url)
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
