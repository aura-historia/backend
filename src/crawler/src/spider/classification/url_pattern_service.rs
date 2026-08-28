use listing_source_core::ListingSourceId;
use std::sync::Arc;

use crate::review::model::ARTIFACT_URL_PATTERN;
use crate::review::repository::CrawlerReviewRepository;
use crate::spider::classification::url_classification_service::{
    UrlClassificationError, UrlClassificationService,
};
use crate::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepository;
use crate::spider::utils::url::extract_shop_base_url;
use regex::Regex;
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum UrlPatternServiceError {
    #[error("Invalid shop URL '{shop_url}': {source}")]
    InvalidShopUrl {
        shop_url: String,
        source: listing_source_core::InvalidDomain,
    },

    #[error(transparent)]
    Repository(#[from] sqlx::Error),

    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error(transparent)]
    Classification(#[from] UrlClassificationError),

    #[error(
        "URL pattern generation is blocked pending review '{review_id}' for shop '{listing_source_id}'"
    )]
    PendingReview {
        listing_source_id: ListingSourceId,
        review_id: uuid::Uuid,
    },
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlPatternService: Send + Sync {
    /// Loads the persisted pattern for `listing_source_id` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    async fn load_pattern_for_shop(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Persists `pattern` for `listing_source_id` with its `shop_url` origin as domain.
    async fn save_pattern_for_shop(
        &self,
        listing_source_id: &ListingSourceId,
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
        listing_source_id: &ListingSourceId,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Marks the shop as crawled now.
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        shop_url: &str,
    ) -> Result<(), UrlPatternServiceError>;
}

pub struct UrlPatternServiceImpl {
    repository: Arc<dyn ListingSourceUrlPatternRepository>,
    classification_service: Box<dyn UrlClassificationService>,
    review_repository: Option<CrawlerReviewRepository>,
    review_required: bool,
}

impl UrlPatternServiceImpl {
    pub fn new(
        repository: Arc<dyn ListingSourceUrlPatternRepository>,
        classification_service: Box<dyn UrlClassificationService>,
    ) -> Self {
        Self {
            repository,
            classification_service,
            review_repository: None,
            review_required: false,
        }
    }

    pub fn new_with_review(
        repository: Arc<dyn ListingSourceUrlPatternRepository>,
        classification_service: Box<dyn UrlClassificationService>,
        review_repository: CrawlerReviewRepository,
        review_required: bool,
    ) -> Self {
        Self {
            repository,
            classification_service,
            review_repository: Some(review_repository),
            review_required,
        }
    }
}

#[async_trait::async_trait]
impl UrlPatternService for UrlPatternServiceImpl {
    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id))]
    async fn load_pattern_for_shop(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        let record = self.repository.find_pattern(listing_source_id).await?;

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
        listing_source_id: &ListingSourceId,
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
            .save_pattern(listing_source_id, &extracted_domain, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        skip(self, urls),
        fields(listing_source_id = %listing_source_id, shop_url = %shop_url, url_count = urls.len())
    )]
    async fn classify_and_save(
        &self,
        listing_source_id: &ListingSourceId,
        shop_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        if self.review_required
            && let Some(review_repository) = &self.review_repository
            && review_repository
                .has_pending_review(listing_source_id, ARTIFACT_URL_PATTERN)
                .await?
        {
            let review_id = review_repository
                .latest_pending_review_id(listing_source_id, ARTIFACT_URL_PATTERN)
                .await?
                .unwrap_or_else(uuid::Uuid::nil);
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        self.repository
            .increment_listing_source_llm_calls(listing_source_id, 1)
            .await?;
        let pattern = self
            .classification_service
            .find_product_url_pattern(urls)
            .await?;

        if self.review_required
            && let Some(review_repository) = &self.review_repository
        {
            let current_pattern = self.load_pattern_for_shop(listing_source_id).await?;
            let review_id = review_repository
                .create_url_pattern_review(
                    listing_source_id,
                    None,
                    "url_pattern_generation",
                    pattern.as_ref(),
                    urls,
                    current_pattern.as_ref(),
                )
                .await
                .map_err(|err| {
                    UrlPatternServiceError::Repository(sqlx::Error::Protocol(err.to_string()))
                })?;
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        if let Some(ref p) = pattern {
            self.save_pattern_for_shop(listing_source_id, shop_url, p)
                .await?;
            match extract_shop_base_url(shop_url) {
                Ok(extracted_domain) => {
                    debug!(domain = %extracted_domain, "Persisted product URL pattern")
                }
                Err(_) => debug!(domain = %shop_url, "Persisted product URL pattern"),
            }
        }

        Ok(pattern)
    }

    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, shop_url = %shop_url))]
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        shop_url: &str,
    ) -> Result<(), UrlPatternServiceError> {
        let extracted_domain = extract_shop_base_url(shop_url).map_err(|error| {
            UrlPatternServiceError::InvalidShopUrl {
                shop_url: shop_url.to_string(),
                source: error,
            }
        })?;
        self.repository
            .mark_as_crawled(listing_source_id, &extracted_domain)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;

    use crate::spider::classification::url_pattern_repository::ListingSourceUrlPatternRecord;
    use crate::spider::classification::url_pattern_repository::MockListingSourceUrlPatternRepository;

    #[tokio::test]
    async fn should_load_pattern_from_repo_when_available() {
        let mut mock_repo = MockListingSourceUrlPatternRepository::new();
        mock_repo.expect_find_pattern().returning(|_| {
            Box::pin(async {
                Ok(Some(ListingSourceUrlPatternRecord {
                    listing_source_id: uuid::Uuid::new_v4().into(),
                    listing_source_domain: listing_source_core::Domain::try_from("example.com")
                        .unwrap(),
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

        let listing_source_id = uuid::Uuid::new_v4().into();
        let result = service.load_pattern_for_shop(&listing_source_id).await;
        assert!(result.is_ok());
        let pattern = result.unwrap();
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().as_str(), "/product/");
    }

    #[tokio::test]
    async fn should_return_none_when_repo_has_no_pattern() {
        let mut mock_repo = MockListingSourceUrlPatternRepository::new();
        mock_repo
            .expect_find_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let listing_source_id = uuid::Uuid::new_v4().into();
        let result = service.load_pattern_for_shop(&listing_source_id).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn should_save_pattern_to_repo() {
        let mut mock_repo = MockListingSourceUrlPatternRepository::new();
        mock_repo
            .expect_save_pattern()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let regex = Regex::new("/product/").unwrap();
        let listing_source_id = uuid::Uuid::new_v4().into();
        let result = service
            .save_pattern_for_shop(&listing_source_id, "https://example.com", &regex)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_classify_and_save_pattern() {
        let mut mock_repo = MockListingSourceUrlPatternRepository::new();
        mock_repo
            .expect_increment_listing_source_llm_calls()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        mock_repo
            .expect_save_pattern()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        mock_client
            .expect_find_product_url_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new("/product/").unwrap())) }));

        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let listing_source_id = uuid::Uuid::new_v4().into();
        let result = service
            .classify_and_save(
                &listing_source_id,
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
        let mut mock_repo = MockListingSourceUrlPatternRepository::new();
        mock_repo
            .expect_mark_as_crawled()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mock_client =
            crate::spider::classification::url_classification_service::MockUrlClassificationService::new();
        let service = UrlPatternServiceImpl::new(Arc::new(mock_repo), Box::new(mock_client));

        let listing_source_id = uuid::Uuid::new_v4().into();
        let result = service
            .mark_as_crawled(&listing_source_id, "https://example.com")
            .await;
        assert!(result.is_ok());
    }
}
