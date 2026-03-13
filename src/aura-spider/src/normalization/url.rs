use url::Url;

/// Normalizes URLs for stable deduplication and comparison.
///
/// Rules:
/// - preserve query string
/// - remove fragment only
/// - sort query parameters for stable comparison
/// - trim trailing slash on paths (except bare root)
/// - lowercase scheme and host
pub fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();

    let Ok(mut parsed) = Url::parse(trimmed) else {
        return trimmed.trim_end_matches('/').to_string();
    };

    if let Some(query) = parsed.query()
        && !query.is_empty()
    {
        let mut pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        pairs.sort();

        let sorted_query = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&sorted_query));
    }

    parsed.set_fragment(None);

    let normalized = parsed.to_string();

    if normalized.ends_with('/') {
        let without_trailing = normalized.trim_end_matches('/');
        if without_trailing.contains("://") && without_trailing.contains('/') {
            let after_scheme = without_trailing
                .split_once("://")
                .map(|(_, rhs)| rhs)
                .unwrap_or("");
            if after_scheme.contains('/') {
                return without_trailing.to_string();
            }
        }
    }

    normalized
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
