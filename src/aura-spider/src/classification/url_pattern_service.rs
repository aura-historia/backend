use std::sync::Arc;

use regex::Regex;
use tracing::info;

use crate::classification::gemini_client::PatternInferenceClient;
use crate::classification::url_classification_service::find_product_url_pattern;
use crate::classification::url_pattern_repository::ShopUrlPatternRepository;
use crate::error::SpiderError;
use crate::utils::url::extract_shop_base_url;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlPatternService: Send + Sync {
    /// Loads the persisted pattern for `shop_url` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    async fn load_pattern_for_shop_url(&self, shop_url: &str)
    -> Result<Option<Regex>, SpiderError>;

    /// Persists `pattern` for `shop_url`.
    async fn save_pattern_for_shop_url(
        &self,
        shop_url: &str,
        pattern: &Regex,
    ) -> Result<(), SpiderError>;

    /// Asks the inference client to classify a product URL pattern from `urls`, persists the
    /// result when one is found, and returns it.
    ///
    /// This is the fallback used when no stored pattern exists or when
    /// the stored pattern must be refreshed after a failed crawl.
    async fn classify_and_save(
        &self,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, SpiderError>;

    /// Marks the shop as crawled now.
    async fn mark_as_crawled(&self, shop_url: &str) -> Result<(), SpiderError>;
}

pub struct UrlPatternServiceImpl {
    repository: Arc<dyn ShopUrlPatternRepository>,
    inference_client: Box<dyn PatternInferenceClient>,
}

impl UrlPatternServiceImpl {
    pub fn new(
        repository: Arc<dyn ShopUrlPatternRepository>,
        inference_client: Box<dyn PatternInferenceClient>,
    ) -> Self {
        Self {
            repository,
            inference_client,
        }
    }
}

#[async_trait::async_trait]
impl UrlPatternService for UrlPatternServiceImpl {
    async fn load_pattern_for_shop_url(
        &self,
        shop_url: &str,
    ) -> Result<Option<Regex>, SpiderError> {
        let shop_url = extract_shop_base_url(shop_url)?;
        let record = self.repository.find_pattern(&shop_url).await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let Some(raw_pattern) = record.url_pattern else {
            return Ok(None);
        };

        let pattern = Regex::new(&raw_pattern)?;
        Ok(Some(pattern))
    }

    async fn save_pattern_for_shop_url(
        &self,
        shop_url: &str,
        pattern: &Regex,
    ) -> Result<(), SpiderError> {
        let shop_url = extract_shop_base_url(shop_url)?;
        self.repository
            .save_pattern(&shop_url, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    async fn classify_and_save(
        &self,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, SpiderError> {
        let pattern = find_product_url_pattern(self.inference_client.as_ref(), urls).await?;

        if let Some(ref p) = pattern {
            self.save_pattern_for_shop_url(shop_url, p).await?;
            info!(shopUrl = %shop_url, "Persisted product URL pattern");
        }

        Ok(pattern)
    }

    async fn mark_as_crawled(&self, shop_url: &str) -> Result<(), SpiderError> {
        let shop_url = extract_shop_base_url(shop_url)?;
        self.repository.mark_as_crawled(&shop_url).await?;
        Ok(())
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::classification::gemini_client::MockPatternInferenceClient;
    use crate::classification::url_pattern_repository::MockShopUrlPatternRepository;
    use crate::classification::url_pattern_repository::ShopUrlPatternRecord;

    #[tokio::test]
    async fn should_load_pattern_from_repo_when_available() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo.expect_find_pattern().returning(|_| {
            Box::pin(async {
                Ok(Some(ShopUrlPatternRecord {
                    shop_url: "https://example.com".to_string(),
                    url_pattern: Some("/product/".to_string()),
                    last_crawled: None,
                    created: time::OffsetDateTime::now_utc(),
                    updated: time::OffsetDateTime::now_utc(),
                }))
            })
        });

        let mock_client = MockPatternInferenceClient::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let result = service
            .load_pattern_for_shop_url("https://example.com")
            .await;
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().as_str(), "/product/");
    }

    #[tokio::test]
    async fn should_return_none_when_repo_has_no_pattern() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_find_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));

        let mock_client = MockPatternInferenceClient::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let result = service
            .load_pattern_for_shop_url("https://example.com")
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn should_save_pattern_to_repo() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_save_pattern()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mock_client = MockPatternInferenceClient::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let regex = Regex::new("/product/").unwrap();
        let result = service
            .save_pattern_for_shop_url("https://example.com", &regex)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_classify_and_save_pattern() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_save_pattern()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut mock_client = MockPatternInferenceClient::new();
        mock_client
            .expect_infer_product_url_pattern()
            .returning(|_| Box::pin(async { Ok(Some("/product/".to_string())) }));

        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let result = service
            .classify_and_save(
                "https://example.com",
                &["https://example.com/product/1".to_string()],
            )
            .await;
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().as_str(), "/product/");
    }
}
