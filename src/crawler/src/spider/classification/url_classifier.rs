//! The class the crawler stores for a URL. The spider's crawl, the URL sample of a URL-pattern
//! review and the reclassification when a URL-pattern decision goes live classify a URL alike.

use crate::spider::classification::url_metadata::UrlClass;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;
use url::Url;

/// The class of the stored URL `raw_url` under the domain's product `pattern`, or `other` for a
/// URL that does not parse.
pub fn classify_url(raw_url: &str, pattern: Option<&Regex>) -> UrlClass {
    match Url::parse(raw_url) {
        Ok(url) => CrawledUrl::new(url).classify(pattern),
        Err(_) => UrlClass::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = Regex::new(r"/product/").unwrap();

        let class = classify_url("https://example.com/product/42", Some(&pattern));

        assert_eq!(class, UrlClass::ProductListing);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let class = classify_url("https://example.com/impressum", None);

        assert_eq!(class, UrlClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let class = classify_url("https://example.com/collections/modern", None);

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let class = classify_url("https://example.com/random-page", None);

        assert_eq!(class, UrlClass::Other);
    }

    #[test]
    fn should_classify_other_when_url_does_not_parse() {
        let pattern = Regex::new(r"/product/").unwrap();

        assert_eq!(classify_url("/product/42", Some(&pattern)), UrlClass::Other);
    }
}
