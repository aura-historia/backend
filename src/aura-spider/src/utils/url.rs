use url::Url;
use url_normalize::Options;

use crate::error::SpiderError;

pub fn clean_and_normalize_url(raw: &str) -> String {
    if Url::parse(raw.trim()).is_err() {
        return raw.trim().trim_end_matches('/').to_string();
    }

    let mut options = Options::default();
    options.strip_hash = true;
    options.remove_trailing_slash = true;
    options.remove_single_slash = false;
    options.strip_www = false;

    match url_normalize::normalize_url(raw.trim(), &options) {
        Ok(normalized) => normalized,
        Err(_) => raw.trim().trim_end_matches('/').to_string(),
    }
}

pub fn extract_shop_base_url(shop_url: &str) -> Result<String, SpiderError> {
    let parsed = Url::parse(shop_url).map_err(|error| {
        SpiderError::Spider(format!(
            "Invalid shop URL '{shop_url}' while resolving pattern scope: {error}"
        ))
    })?;

    let origin = parsed.origin();
    if origin.is_tuple() {
        Ok(origin.ascii_serialization())
    } else {
        Err(SpiderError::Spider(format!(
            "Shop URL '{shop_url}' has no valid origin for pattern scope"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_normalize_url_when_fragment_exists_for_comparison() {
        let input = "https://www.example.com/product/123?ref=google#reviews";
        let normalized = clean_and_normalize_url(input);
        assert_eq!(normalized, "https://www.example.com/product/123?ref=google");
    }

    #[test]
    fn should_trim_trailing_slash_when_path_is_not_root_for_comparison() {
        let input = "https://www.example.com/products/";
        let normalized = clean_and_normalize_url(input);
        assert_eq!(normalized, "https://www.example.com/products");
    }

    #[test]
    fn should_trim_trailing_slash_with_query_params() {
        let input = "https://www.example.com/products/?q=1";
        let normalized = clean_and_normalize_url(input);
        assert_eq!(normalized, "https://www.example.com/products?q=1");
    }

    #[test]
    fn should_keep_root_slash_when_path_is_root_for_comparison() {
        let input = "https://www.example.com/";
        let normalized = clean_and_normalize_url(input);
        assert_eq!(normalized, "https://www.example.com/");
    }

    #[test]
    fn should_lowercase_scheme_and_host_when_url_is_mixed_case_for_comparison() {
        let input = "HTTPS://WWW.EXAMPLE.COM/Product/123";
        let normalized = clean_and_normalize_url(input);
        assert!(normalized.starts_with("https://www.example.com/"));
    }

    #[test]
    fn should_equalize_query_order_when_parameters_are_reordered_for_deduplication() {
        let first = clean_and_normalize_url("https://www.example.com/product/123?b=2&a=1");
        let second = clean_and_normalize_url("https://www.example.com/product/123?a=1&b=2");
        assert_eq!(first, second);
    }

    #[test]
    fn should_not_change_plain_text_when_input_is_not_a_valid_url() {
        let input = "not a valid url";
        let normalized = clean_and_normalize_url(input);
        assert_eq!(normalized, "not a valid url");
    }

    #[test]
    fn should_return_origin_when_shop_url_has_default_port_for_scope_key() {
        let key = extract_shop_base_url("https://example.com/some/path")
            .expect("shop url should be resolved");
        assert_eq!(key, "https://example.com");
    }

    #[test]
    fn should_return_origin_with_port_when_shop_url_has_explicit_port_for_scope_key() {
        let key = extract_shop_base_url("https://example.com:8443/some/path")
            .expect("shop url should be resolved");
        assert_eq!(key, "https://example.com:8443");
    }

    #[test]
    fn should_return_error_when_shop_url_is_invalid_for_scope_key() {
        let error = extract_shop_base_url("not-a-valid-url").expect_err("invalid url should fail");
        assert!(matches!(error, SpiderError::Spider(_)));
    }
}
