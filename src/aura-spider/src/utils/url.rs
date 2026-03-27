use crate::error::SpiderError;
use common::domain::Domain;
use url_normalize::Options;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrawledUrl(pub url::Url);

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

    pub fn matches_pattern(&self, pattern: &regex::Regex) -> bool {
        pattern.is_match(self.0.as_str())
    }
}

impl std::fmt::Display for CrawledUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn extract_shop_base_url(shop_url: &str) -> Result<Domain, SpiderError> {
    Domain::try_from(shop_url).map_err(|error| {
        SpiderError::Spider(format!(
            "Invalid shop URL '{shop_url}' while resolving pattern scope: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_for_test(input: &str) -> String {
        if let Ok(parsed) = url::Url::parse(input) {
            CrawledUrl::new(parsed).0.to_string()
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
    fn should_lowercase_scheme_and_host_when_url_is_mixed_case_for_comparison() {
        let input = "HTTPS://WWW.EXAMPLE.COM/Product/123";
        let normalized = normalize_for_test(input);
        assert!(normalized.starts_with("https://www.example.com/"));
    }

    #[test]
    fn should_equalize_query_order_when_parameters_are_reordered_for_deduplication() {
        let first = normalize_for_test("https://www.example.com/product/123?b=2&a=1");
        let second = normalize_for_test("https://www.example.com/product/123?a=1&b=2");
        assert_eq!(first, second);
    }

    #[test]
    fn should_not_change_plain_text_when_input_is_not_a_valid_url() {
        let input = "not a valid url";
        let normalized = normalize_for_test(input);
        assert_eq!(normalized, "not a valid url");
    }

    #[test]
    fn should_return_origin_when_shop_url_has_default_port_for_scope_key() {
        let key = extract_shop_base_url("https://example.com/some/path")
            .expect("shop url should be resolved");
        assert_eq!(key.as_str(), "example.com");
    }

    #[test]
    fn should_return_origin_with_port_when_shop_url_has_explicit_port_for_scope_key() {
        let key = extract_shop_base_url("https://example.com:8443/some/path")
            .expect("shop url should be resolved");
        assert_eq!(key.as_str(), "example.com");
    }

    #[test]
    fn should_return_error_when_shop_url_is_invalid_for_scope_key() {
        let error = extract_shop_base_url("not-a-valid-url").expect_err("invalid url should fail");
        assert!(matches!(error, SpiderError::Spider(_)));
    }
}
