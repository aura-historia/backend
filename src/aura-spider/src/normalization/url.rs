use url::Url;
use crate::error::SpiderError;

/// Normalizes URLs for stable deduplication and comparison.
///
/// Rules:
/// - preserve query string
/// - remove fragment only
/// - sort query parameters for stable comparison
/// - trim trailing slash on paths (except bare root)
/// - lowercase scheme and host
pub fn normalize_url(raw: &str) -> String {
    let Ok(mut parsed) = Url::parse(raw.trim()) else {
        return raw.trim().trim_end_matches('/').to_string();
    };

    // Query Parameter Handling
    let blocklist = [
        "add-to-cart",
        "wp_customize",
        "replytocom",
        "utm_source",
        "fbclid",
    ];

    let mut pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !blocklist.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    if pairs.is_empty() {
        parsed.set_query(None);
    } else {
        pairs.sort();
        let sorted = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&sorted));
    }

    // Standard Cleaning
    parsed.set_fragment(None);

    // Remove trailing slash from path if it exists and is not root
    if parsed.path() != "/" {
        let path = parsed.path().to_string();
        if path.ends_with('/') {
            let new_path = path.trim_end_matches('/');
            parsed.set_path(new_path);
        }
    }

    parsed.to_string()
}


/// Normalizes a shop URL to its origin (scheme + host + optional port) used for scoping/persistence.
pub fn normalize_shop_url(shop_url: &str) -> Result<String, SpiderError> {
    let parsed = Url::parse(shop_url).map_err(|error| {
        SpiderError::Spider(format!(
            "Invalid shop URL '{shop_url}' while resolving pattern scope: {error}"
        ))
    })?;

    let host = parsed.host_str().ok_or_else(|| {
        SpiderError::Spider(format!(
            "Shop URL '{shop_url}' has no host for pattern scope"
        ))
    })?;

    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = host.to_ascii_lowercase();

    Ok(match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn should_normalize_url_when_fragment_exists_for_comparison() {
        let input = "https://www.example.com/product/123?ref=google#reviews";

        let normalized = normalize_url(input);

        assert_eq!(normalized, "https://www.example.com/product/123?ref=google");
    }

    #[test]
    fn should_trim_trailing_slash_when_path_is_not_root_for_comparison() {
        let input = "https://www.example.com/products/";

        let normalized = normalize_url(input);

        assert_eq!(normalized, "https://www.example.com/products");
    }

    #[test]
    fn should_trim_trailing_slash_with_query_params() {
        // Test new behavior: ensures slash is removed even with query params
        let input = "https://www.example.com/products/?q=1";
        let normalized = normalize_url(input);
        assert_eq!(normalized, "https://www.example.com/products?q=1");
    }

    #[test]
    fn should_keep_root_slash_when_path_is_root_for_comparison() {
        let input = "https://www.example.com/";

        let normalized = normalize_url(input);

        assert_eq!(normalized, "https://www.example.com/");
    }

    #[test]
    fn should_lowercase_scheme_and_host_when_url_is_mixed_case_for_comparison() {
        let input = "HTTPS://WWW.EXAMPLE.COM/Product/123";

        let normalized = normalize_url(input);

        assert!(normalized.starts_with("https://www.example.com/"));
    }

    #[test]
    fn should_equalize_query_order_when_parameters_are_reordered_for_deduplication() {
        let first = normalize_url("https://www.example.com/product/123?b=2&a=1");
        let second = normalize_url("https://www.example.com/product/123?a=1&b=2");

        assert_eq!(first, second);
    }

    #[test]
    fn should_not_change_plain_text_when_input_is_not_a_valid_url() {
        let input = "not a valid url";

        let normalized = normalize_url(input);

        assert_eq!(normalized, "not a valid url");
    }
}
