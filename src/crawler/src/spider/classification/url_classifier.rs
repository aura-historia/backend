//! The class the crawler stores for a URL. The spider's crawl, the URL sample of a URL-pattern
//! review and the reclassification when a URL-pattern decision goes live classify a URL alike,
//! with the classifier the authoring mode selects.

use crate::authoring::CrawlerAuthoringMode;
use crate::spider::classification::url_metadata::UrlClass;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;
use url::Url;

/// How the crawler classifies a URL: by the domain's product pattern and, for the URLs it does
/// not take, by keywords in the URL, or by the pattern alone. The operator contract is in
/// `docs/crawler/operations.md` (review agents).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UrlClassifier {
    /// A URL the pattern takes is a product URL; any other one is a category, imprint or info
    /// URL when its spelling contains one of their keywords, else `other`.
    #[default]
    PatternAndKeywords,
    /// A URL the pattern takes is a product URL; any other one stays `other`, unclassified.
    /// Without a pattern every URL stays unclassified: the crawler does not guess what a page is.
    PatternOnly,
}

impl UrlClassifier {
    /// The classifier of `mode`: in `agent` mode Claude decides what a product page is, so the
    /// crawler applies the approved pattern and nothing else; the other modes keep the keywords.
    pub const fn for_authoring(mode: CrawlerAuthoringMode) -> Self {
        match mode {
            CrawlerAuthoringMode::Agent => Self::PatternOnly,
            CrawlerAuthoringMode::Auto
            | CrawlerAuthoringMode::Review
            | CrawlerAuthoringMode::Manual => Self::PatternAndKeywords,
        }
    }

    /// The class of the stored URL `raw_url` under the domain's product `pattern`, or `other` for
    /// a URL that does not parse.
    pub fn classify(self, raw_url: &str, pattern: Option<&Regex>) -> UrlClass {
        let Ok(url) = Url::parse(raw_url) else {
            return UrlClass::Other;
        };
        let url = CrawledUrl::new(url);
        match self {
            Self::PatternAndKeywords => url.classify(pattern),
            Self::PatternOnly if pattern.is_some_and(|pattern| url.matches_pattern(pattern)) => {
                UrlClass::ProductListing
            }
            Self::PatternOnly => UrlClass::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYWORDS: UrlClassifier = UrlClassifier::PatternAndKeywords;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = Regex::new(r"/product/").unwrap();

        let class = KEYWORDS.classify("https://example.com/product/42", Some(&pattern));

        assert_eq!(class, UrlClass::ProductListing);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let class = KEYWORDS.classify("https://example.com/impressum", None);

        assert_eq!(class, UrlClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let class = KEYWORDS.classify("https://example.com/collections/modern", None);

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let class = KEYWORDS.classify("https://example.com/random-page", None);

        assert_eq!(class, UrlClass::Other);
    }

    #[test]
    fn should_classify_other_when_url_does_not_parse() {
        let pattern = Regex::new(r"/product/").unwrap();

        for classifier in [
            UrlClassifier::PatternAndKeywords,
            UrlClassifier::PatternOnly,
        ] {
            assert_eq!(
                classifier.classify("/product/42", Some(&pattern)),
                UrlClass::Other,
                "{classifier:?}"
            );
        }
    }

    /// Without the keywords, a shop whose domain or paths contain `shop`, `category` or `about`
    /// reads as nothing until a pattern is approved, and then only as products and the rest.
    #[test]
    fn should_leave_every_url_the_pattern_does_not_take_unclassified_by_the_pattern_alone() {
        let pattern = Regex::new(r"/item/[^/]+$").unwrap();
        let urls = [
            "https://antique-shop.example/item/oak-chair",
            "https://antique-shop.example/category/chairs",
            "https://antique-shop.example/impressum",
            "https://antique-shop.example/about-us",
        ];

        let unclassified: Vec<UrlClass> = urls
            .iter()
            .map(|url| UrlClassifier::PatternOnly.classify(url, None))
            .collect();
        let by_pattern: Vec<UrlClass> = urls
            .iter()
            .map(|url| UrlClassifier::PatternOnly.classify(url, Some(&pattern)))
            .collect();

        assert_eq!(unclassified, vec![UrlClass::Other; 4]);
        assert_eq!(
            by_pattern,
            vec![
                UrlClass::ProductListing,
                UrlClass::Other,
                UrlClass::Other,
                UrlClass::Other
            ]
        );
        assert_eq!(
            KEYWORDS.classify(urls[0], None),
            UrlClass::Category,
            "the keywords read the shop's domain as a category"
        );
    }

    #[test]
    fn should_classify_by_the_pattern_alone_only_in_agent_mode() {
        for mode in CrawlerAuthoringMode::ALL {
            assert_eq!(
                UrlClassifier::for_authoring(mode),
                if mode == CrawlerAuthoringMode::Agent {
                    UrlClassifier::PatternOnly
                } else {
                    UrlClassifier::PatternAndKeywords
                },
                "{mode:?}"
            );
        }
    }
}
