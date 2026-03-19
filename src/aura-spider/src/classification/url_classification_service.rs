use std::collections::HashSet;

use regex::Regex;
use tracing::{debug, info, warn};

use crate::classification::gemini_client::PatternInferenceClient;
use crate::error::SpiderError;
use crate::url::normalize_url;

/// Uses a PatternInferenceClient to infer a product URL regex and compiles it.
pub async fn find_product_url_pattern(
    client: &dyn PatternInferenceClient,
    all_urls: &[String],
) -> Result<Option<Regex>, SpiderError> {
    info!(
        urlCount = all_urls.len(),
        "Analyzing crawled URLs with PatternInferenceClient"
    );

    match client.infer_product_url_pattern(all_urls).await {
        Ok(Some(pattern)) => match Regex::new(&pattern) {
            Ok(regex) => {
                info!(pattern = %pattern, "Client returned a valid URL pattern");
                Ok(Some(regex))
            }
            Err(error) => {
                warn!(
                    pattern = %pattern,
                    error = %error,
                    "Client returned an invalid regex pattern"
                );
                Ok(None)
            }
        },
        Ok(None) => {
            info!("Client found no consistent product URL pattern");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Checks whether a single URL matches the current product URL pattern.
pub fn matches_product_pattern(pattern: &Option<Regex>, url: &str) -> bool {
    pattern.as_ref().is_some_and(|regex| regex.is_match(url))
}

/// Applies the inferred product URL pattern to all crawled URLs and returns unique matches.
pub fn filter_product_urls(
    pattern: &Option<Regex>,
    all_urls: &[String],
) -> Result<Vec<String>, SpiderError> {
    let Some(regex) = pattern else {
        return Err(SpiderError::NoProducts(
            "No pattern available - classification skipped".to_string(),
        ));
    };

    info!(
        urlCount = all_urls.len(),
        "Applying URL pattern to crawled URLs"
    );
    let matches: Vec<String> = all_urls
        .iter()
        .filter(|url| regex.is_match(url))
        .cloned()
        .collect();

    debug!(matchCount = matches.len(), "Finished applying URL pattern");

    if matches.is_empty() {
        return Err(SpiderError::NoProducts(
            "Gemini pattern matched 0 URLs".to_string(),
        ));
    }

    Ok(dedupe_urls(matches))
}

fn dedupe_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::<String>::new();
    let mut unique = Vec::new();

    for raw in urls {
        let normalized = normalize_url(&raw);
        if seen.insert(normalized.clone()) {
            unique.push(normalized);
        }
    }

    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_match_url_when_pattern_matches_for_product_page() {
        let pattern = Regex::new(r"/product/\d+").ok();

        assert!(matches_product_pattern(
            &pattern,
            "https://example.com/product/123"
        ));
        assert!(!matches_product_pattern(
            &pattern,
            "https://example.com/about"
        ));
    }

    #[test]
    fn should_not_match_url_when_pattern_is_missing_for_any_page() {
        let pattern: Option<Regex> = None;

        assert!(!matches_product_pattern(
            &pattern,
            "https://example.com/product/123"
        ));
    }

    #[test]
    fn should_dedupe_urls_when_normalized_values_match_for_duplicates() {
        let urls = vec![
            "https://example.com/product/1?a=1&b=2".to_string(),
            "https://example.com/product/1?b=2&a=1".to_string(),
            "https://example.com/product/2".to_string(),
        ];

        let deduped = dedupe_urls(urls);

        assert_eq!(deduped.len(), 2);
        assert!(deduped.contains(&"https://example.com/product/1?a=1&b=2".to_string()));
        assert!(deduped.contains(&"https://example.com/product/2".to_string()));
    }

    #[test]
    fn should_return_error_when_pattern_is_missing_for_filtering() {
        let pattern: Option<Regex> = None;
        let all_urls = vec!["https://example.com/product/1".to_string()];

        let result = filter_product_urls(&pattern, &all_urls);

        assert!(result.is_err());
    }

    #[test]
    fn should_return_error_when_pattern_matches_nothing_for_filtering() {
        let pattern = Regex::new(r"/nomatch").ok();
        let all_urls = vec!["https://example.com/product/1".to_string()];

        let result = filter_product_urls(&pattern, &all_urls);

        assert!(result.is_err());
        match result {
            Err(SpiderError::NoProducts(msg)) => assert!(msg.contains("pattern matched 0")),
            _ => panic!("Expected NoProducts error"),
        }
    }

    #[test]
    fn should_return_products_when_pattern_matches_for_filtering() {
        let pattern = Regex::new(r"/product/\d+").ok();
        let all_urls = vec![
            "https://example.com/product/1".to_string(),
            "https://example.com/product/2".to_string(),
        ];

        let result = filter_product_urls(&pattern, &all_urls);

        assert!(result.is_ok());
        let products = result.expect("filtering should return matched products");
        assert_eq!(products.len(), 2);
    }

    #[tokio::test]
    async fn should_return_pattern_when_client_infers_valid_regex() {
        let mut mock_client =
            crate::classification::gemini_client::MockPatternInferenceClient::new();
        mock_client
            .expect_infer_product_url_pattern()
            .returning(|_| Box::pin(async { Ok(Some(r"/product/\d+".to_string())) }));

        let urls = vec!["https://example.com/product/1".to_string()];
        let result = find_product_url_pattern(&mock_client, &urls).await;

        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().as_str(), r"/product/\d+");
    }

    #[tokio::test]
    async fn should_return_none_when_client_infers_invalid_regex() {
        let mut mock_client =
            crate::classification::gemini_client::MockPatternInferenceClient::new();
        mock_client
            .expect_infer_product_url_pattern()
            .returning(|_| Box::pin(async { Ok(Some(r"[invalid_regex".to_string())) }));

        let urls = vec!["https://example.com/product/1".to_string()];
        let result = find_product_url_pattern(&mock_client, &urls).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn should_return_none_when_client_finds_no_pattern() {
        let mut mock_client =
            crate::classification::gemini_client::MockPatternInferenceClient::new();
        mock_client
            .expect_infer_product_url_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));

        let urls = vec!["https://example.com/product/1".to_string()];
        let result = find_product_url_pattern(&mock_client, &urls).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn should_return_error_when_client_fails() {
        let mut mock_client =
            crate::classification::gemini_client::MockPatternInferenceClient::new();
        mock_client
            .expect_infer_product_url_pattern()
            .returning(|_| Box::pin(async { Err(SpiderError::Gemini("error".to_string())) }));

        let urls = vec!["https://example.com/product/1".to_string()];
        let result = find_product_url_pattern(&mock_client, &urls).await;

        assert!(result.is_err());
    }
}
