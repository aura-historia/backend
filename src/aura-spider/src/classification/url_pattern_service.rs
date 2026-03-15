use std::sync::Arc;

use regex::Regex;
use tracing::info;

use crate::classification::gemini_client::GeminiClient;
use crate::classification::url_classification_service::find_product_url_pattern;
use crate::classification::url_pattern_repository::ShopUrlPatternRepository;
use crate::error::SpiderError;
use crate::normalization::url::normalize_shop_url;

pub struct UrlPatternService {
    repository: Arc<dyn ShopUrlPatternRepository>,
}

impl UrlPatternService {
    pub fn new(repository: Arc<dyn ShopUrlPatternRepository>) -> Self {
        Self { repository }
    }

    /// Loads the persisted pattern for `shop_url` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    pub async fn load_pattern_for_shop_url(
        &self,
        shop_url: &str,
    ) -> Result<Option<Regex>, SpiderError> {
        let shop_url = normalize_shop_url(shop_url)?;
        let record = self.repository.find_pattern(&shop_url).await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let Some(raw_pattern) = record.pattern else {
            return Ok(None);
        };

        let pattern = Regex::new(&raw_pattern)?;
        Ok(Some(pattern))
    }

    /// Persists `pattern` for `shop_url`.
    pub async fn save_pattern_for_shop_url(
        &self,
        shop_url: &str,
        pattern: &Regex,
    ) -> Result<(), SpiderError> {
        let shop_url = normalize_shop_url(shop_url)?;
        self.repository
            .save_pattern(&shop_url, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    /// Asks Gemini to classify a product URL pattern from `urls`, persists the
    /// result when one is found, and returns it.
    ///
    /// This is the Gemini fallback used when no stored pattern exists or when
    /// the stored pattern must be refreshed after a failed crawl.
    pub async fn classify_and_save(
        &self,
        shop_url: &str,
        urls: &[String],
        gemini_client: &GeminiClient,
    ) -> Result<Option<Regex>, SpiderError> {
        let pattern = find_product_url_pattern(gemini_client, urls).await?;

        if let Some(ref p) = pattern {
            self.save_pattern_for_shop_url(shop_url, p).await?;
            info!(shopUrl = %shop_url, "Persisted product URL pattern");
        }

        Ok(pattern)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_origin_when_shop_url_has_default_port_for_scope_key() {
        let key = normalize_shop_url("https://example.com/some/path")
            .expect("shop url should be resolved");

        assert_eq!(key, "https://example.com");
    }

    #[test]
    fn should_return_origin_with_port_when_shop_url_has_explicit_port_for_scope_key() {
        let key = normalize_shop_url("https://example.com:8443/some/path")
            .expect("shop url should be resolved");

        assert_eq!(key, "https://example.com:8443");
    }

    #[test]
    fn should_return_error_when_shop_url_is_invalid_for_scope_key() {
        let error = normalize_shop_url("not-a-valid-url")
            .expect_err("invalid url should fail");

        assert!(matches!(error, SpiderError::Spider(_)));
    }
}
