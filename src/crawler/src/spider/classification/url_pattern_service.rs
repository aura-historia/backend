use std::sync::Arc;

use regex::Regex;
use thiserror::Error;
use tracing::info;

use crate::spider::classification::url_classification_service::{
    UrlClassificationError, UrlClassificationService,
};
use crate::spider::classification::url_pattern_repository::ShopUrlPatternRepository;
use crate::spider::utils::url::extract_shop_base_url;
use common::shop_id::ShopId;

#[derive(Debug, Error)]
pub enum UrlPatternServiceError {
    #[error("Invalid shop URL '{shop_url}': {source}")]
    InvalidShopUrl {
        shop_url: String,
        source: common::domain::NoDomainError,
    },

    #[error(transparent)]
    Repository(#[from] sqlx::Error),

    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error(transparent)]
    Classification(#[from] UrlClassificationError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlPatternService: Send + Sync {
    /// Loads the persisted pattern for `shop_id` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    async fn load_pattern_for_shop(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Persists `pattern` for `shop_id` with its `shop_url` origin as domain.
    async fn save_pattern_for_shop(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError>;

    /// Asks the inference client to classify a product URL pattern from `urls`, persists the
    /// result when one is found, and returns it.
    ///
    /// This is the fallback used when no stored pattern exists or when
    /// the stored pattern must be refreshed after a failed crawl.
    async fn classify_and_save(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Marks the shop as crawled now.
    async fn mark_as_crawled(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<(), UrlPatternServiceError>;

    /// Attempts to acquire a lock for this shop crawl.
    async fn try_lock_shop(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<bool, UrlPatternServiceError>;

    /// Releases a previously acquired shop crawl lock.
    async fn unlock_shop(&self, shop_id: &ShopId) -> Result<(), UrlPatternServiceError>;
}

pub struct UrlPatternServiceImpl {
    repository: Arc<dyn ShopUrlPatternRepository>,
    classification_service: Box<dyn UrlClassificationService>,
}

impl UrlPatternServiceImpl {
    pub fn new(
        repository: Arc<dyn ShopUrlPatternRepository>,
        classification_service: Box<dyn UrlClassificationService>,
    ) -> Self {
        Self {
            repository,
            classification_service,
        }
    }
}

#[async_trait::async_trait]
impl UrlPatternService for UrlPatternServiceImpl {
    async fn load_pattern_for_shop(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        let record = self.repository.find_pattern(shop_id).await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let Some(raw_pattern) = record.url_pattern else {
            return Ok(None);
        };

        let pattern = Regex::new(&raw_pattern)?;
        Ok(Some(pattern))
    }

    async fn save_pattern_for_shop(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError> {
        let extracted_domain = extract_shop_base_url(shop_url).map_err(|error| {
            UrlPatternServiceError::InvalidShopUrl {
                shop_url: shop_url.to_string(),
                source: error,
            }
        })?;
        self.repository
            .save_pattern(shop_id, &extracted_domain, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    async fn classify_and_save(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        let pattern = self
            .classification_service
            .find_product_url_pattern(shop_url, urls)
            .await?;

        if let Some(ref p) = pattern {
            self.save_pattern_for_shop(shop_id, shop_url, p).await?;
            info!(shopId = %shop_id, "Persisted product URL pattern");
        }

        Ok(pattern)
    }

    async fn mark_as_crawled(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<(), UrlPatternServiceError> {
        let extracted_domain = extract_shop_base_url(shop_url).map_err(|error| {
            UrlPatternServiceError::InvalidShopUrl {
                shop_url: shop_url.to_string(),
                source: error,
            }
        })?;
        self.repository
            .mark_as_crawled(shop_id, &extracted_domain)
            .await?;
        Ok(())
    }

    async fn try_lock_shop(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<bool, UrlPatternServiceError> {
        let extracted_domain = extract_shop_base_url(shop_url).map_err(|error| {
            UrlPatternServiceError::InvalidShopUrl {
                shop_url: shop_url.to_string(),
                source: error,
            }
        })?;
        Ok(self
            .repository
            .try_lock_shop(shop_id, &extracted_domain)
            .await?)
    }

    async fn unlock_shop(&self, shop_id: &ShopId) -> Result<(), UrlPatternServiceError> {
        self.repository.unlock_shop(shop_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;

    use crate::spider::classification::url_pattern_repository::MockShopUrlPatternRepository;
    use crate::spider::classification::url_pattern_repository::ShopUrlPatternRecord;

    #[tokio::test]
    async fn should_load_pattern_from_repo_when_available() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo.expect_find_pattern().returning(|_| {
            Box::pin(async {
                Ok(Some(ShopUrlPatternRecord {
                    shop_id: uuid::Uuid::new_v4().into(),
                    shop_domain: common::domain::Domain::try_from("example.com").unwrap(),
                    url_pattern: Some("/product/".to_string()),
                    last_crawled: None,
                    created: time::OffsetDateTime::now_utc(),
                    updated: time::OffsetDateTime::now_utc(),
                }))
            })
        });

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service.load_pattern_for_shop(&shop_id).await;
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

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service.load_pattern_for_shop(&shop_id).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn should_save_pattern_to_repo() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_save_pattern()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let regex = Regex::new("/product/").unwrap();
        let shop_id = uuid::Uuid::new_v4().into();
        let result = service
            .save_pattern_for_shop(&shop_id, "https://example.com", &regex)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_classify_and_save_pattern() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_save_pattern()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        mock_client
            .expect_find_product_url_pattern()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new("/product/").unwrap())) }));

        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service
            .classify_and_save(
                &shop_id,
                "https://example.com",
                &["https://example.com/product/1".to_string()],
            )
            .await;
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().as_str(), "/product/");
    }

    #[tokio::test]
    async fn should_mark_as_crawled_in_repo() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_mark_as_crawled()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service
            .mark_as_crawled(&shop_id, "https://example.com")
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_try_lock_shop_in_repo() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_try_lock_shop()
            .returning(|_, _| Box::pin(async { Ok(true) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service.try_lock_shop(&shop_id, "https://example.com").await;
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn should_unlock_shop_in_repo() {
        let mut mock_repo = MockShopUrlPatternRepository::new();
        mock_repo
            .expect_unlock_shop()
            .returning(|_| Box::pin(async { Ok(()) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let shop_id = uuid::Uuid::new_v4().into();
        let result = service.unlock_shop(&shop_id).await;
        assert!(result.is_ok());
    }
}
