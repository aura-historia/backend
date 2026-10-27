use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use std::sync::Arc;

use crate::review::page_coverage::IncompleteCoverage;
use crate::review::repository::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::spider::classification::url_classification_service::{
    UrlClassificationError, UrlClassificationService,
};
use crate::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, UrlPatternState,
};

use crate::review::model::{ReviewActor, UrlPatternDecision};
use regex::Regex;
use thiserror::Error;
use tracing::{debug, info};

/// Review reason for a URL pattern an operator has to write because LLM calls are disabled.
pub const MANUAL_URL_PATTERN_REASON: &str = "manual_url_pattern_required";

#[derive(Debug, Error)]
pub enum UrlPatternServiceError {
    #[error(transparent)]
    Repository(#[from] sqlx::Error),

    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error(transparent)]
    Classification(#[from] UrlClassificationError),

    #[error(transparent)]
    Review(#[from] ReviewRepositoryError),

    #[error(
        "URL pattern generation is blocked pending review '{review_id}' for ListingSource '{listing_source_id}'"
    )]
    PendingReview {
        listing_source_id: ListingSourceId,
        review_id: CrawlerReviewId,
    },
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlPatternService: Send + Sync {
    /// Loads the persisted pattern for `listing_source_id` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    async fn load_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    async fn save_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError>;

    /// Clears a completed classification so a later crawl may infer again.
    async fn reset_pattern_classification(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError>;

    /// Asks the inference client to classify a product URL pattern from `urls`, persists the
    /// result when one is found, and returns it.
    ///
    /// This is the fallback used when no stored pattern exists or when
    /// the stored pattern must be refreshed after a failed crawl. `crawl_pattern` is the pattern
    /// the crawl classifies with; a stored pattern that differs from it was stored while the crawl
    /// ran and is returned as-is, without inference or review.
    async fn classify_and_save(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        crawl_pattern: Option<&Regex>,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Marks one configured crawler domain as crawled now.
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError>;
}

/// Where a domain's product URL pattern comes from.
pub enum UrlPatternAuthoring {
    /// The LLM infers the pattern. With a review repository, the inferred pattern is recorded as
    /// a review and waits for approval, or, with `apply_directly`, is approved at once. A pending
    /// review for the domain blocks inference either way.
    Inferred {
        classification_service: Box<dyn UrlClassificationService>,
        review_repository: Option<CrawlerReviewRepository>,
        apply_directly: bool,
    },
    /// An operator writes the pattern: classification opens an undecided review and makes no
    /// LLM call.
    Manual {
        review_repository: CrawlerReviewRepository,
    },
}

impl UrlPatternAuthoring {
    fn review_repository(&self) -> Option<&CrawlerReviewRepository> {
        match self {
            Self::Inferred {
                review_repository, ..
            } => review_repository.as_ref(),
            Self::Manual { review_repository } => Some(review_repository),
        }
    }
}

pub struct UrlPatternServiceImpl {
    repository: Arc<dyn ListingSourceUrlPatternRepository>,
    authoring: UrlPatternAuthoring,
}

impl UrlPatternServiceImpl {
    pub fn new(
        repository: Arc<dyn ListingSourceUrlPatternRepository>,
        authoring: UrlPatternAuthoring,
    ) -> Self {
        Self {
            repository,
            authoring,
        }
    }

    async fn open_review(
        &self,
        review_repository: &CrawlerReviewRepository,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        reason: &str,
        decision: UrlPatternDecision,
        urls: &[String],
    ) -> Result<CrawlerReviewId, UrlPatternServiceError> {
        let current_pattern = self
            .load_pattern_for_domain(listing_source_id, domain_id)
            .await?;
        Ok(review_repository
            .create_url_pattern_review(
                listing_source_id,
                domain_id,
                reason,
                decision,
                urls,
                current_pattern.as_ref(),
            )
            .await?)
    }
}

#[async_trait::async_trait]
impl UrlPatternService for UrlPatternServiceImpl {
    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id))]
    async fn load_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        let record = self
            .repository
            .find_pattern(listing_source_id, domain_id)
            .await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let Some(raw_pattern) = record.url_pattern else {
            return Ok(None);
        };

        let pattern = Regex::new(&raw_pattern)?;
        Ok(Some(pattern))
    }

    async fn save_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .save_pattern(listing_source_id, domain_id, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    async fn reset_pattern_classification(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .save_pattern(listing_source_id, domain_id, None)
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        skip(self, crawl_pattern, urls),
        fields(listing_source_id = %listing_source_id, domain_id = %domain_id, crawl_root_url = %crawl_root_url, url_count = urls.len())
    )]
    async fn classify_and_save(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        crawl_pattern: Option<&Regex>,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        if let Some(record) = self
            .repository
            .find_pattern(listing_source_id, domain_id)
            .await?
        {
            match (record.url_pattern_state, record.url_pattern) {
                (UrlPatternState::NoPattern, _) => return Ok(None),
                (UrlPatternState::Matched, Some(stored))
                    if crawl_pattern.is_none_or(|pattern| pattern.as_str() != stored) =>
                {
                    debug!(crawl_root_url, domain_id = %domain_id, "Using product URL pattern stored during the crawl");
                    return Ok(Some(Regex::new(&stored)?));
                }
                _ => {}
            }
        }

        if let Some(review_repository) = self.authoring.review_repository()
            && let Some(review_id) = review_repository
                .latest_pending_url_pattern_review_id(listing_source_id, domain_id)
                .await?
        {
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        let (classification_service, review_repository, apply_directly) = match &self.authoring {
            UrlPatternAuthoring::Inferred {
                classification_service,
                review_repository,
                apply_directly,
            } => (classification_service, review_repository, *apply_directly),
            UrlPatternAuthoring::Manual { review_repository } => {
                let review_id = self
                    .open_review(
                        review_repository,
                        listing_source_id,
                        domain_id,
                        MANUAL_URL_PATTERN_REASON,
                        UrlPatternDecision::Undecided,
                        urls,
                    )
                    .await?;
                info!(
                    review_id = %review_id,
                    "Opened manual URL pattern review; crawl waits for an operator pattern"
                );
                return Err(UrlPatternServiceError::PendingReview {
                    listing_source_id: *listing_source_id,
                    review_id,
                });
            }
        };

        self.repository
            .increment_listing_source_llm_calls(listing_source_id, 1)
            .await?;
        let pattern = classification_service
            .find_product_url_pattern(urls)
            .await?;

        if let Some(review_repository) = review_repository {
            let review_id = self
                .open_review(
                    review_repository,
                    listing_source_id,
                    domain_id,
                    "url_pattern_generation",
                    UrlPatternDecision::inferred(pattern.as_ref()),
                    urls,
                )
                .await?;
            if apply_directly {
                // Approval stores the pattern, so the review records what became active. A URL
                // pattern has no page coverage to acknowledge.
                review_repository
                    .approve_review(
                        review_id,
                        ReviewActor::System,
                        Some("Applied directly by CRAWLER_AUTHORING_MODE=auto"),
                        IncompleteCoverage::Refuse,
                    )
                    .await?;
                debug!(crawl_root_url, domain_id = %domain_id, review_id = %review_id, "Applied inferred product URL pattern");
                return Ok(pattern);
            }
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        match pattern.as_ref() {
            Some(pattern) => {
                self.save_pattern_for_domain(listing_source_id, domain_id, pattern)
                    .await?;
                debug!(crawl_root_url, domain_id = %domain_id, "Persisted product URL pattern");
            }
            None => {
                self.repository
                    .save_no_pattern(listing_source_id, domain_id)
                    .await?;
                debug!(crawl_root_url, domain_id = %domain_id, "Persisted no product URL pattern");
            }
        }

        Ok(pattern)
    }

    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, domain_id = %domain_id))]
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .mark_as_crawled(listing_source_id, domain_id)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::classification::url_classification_service::MockUrlClassificationService;
    use crate::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
    use crate::spider::classification::url_pattern_repository::{
        ListingSourceUrlPatternRecord, MockListingSourceUrlPatternRepository,
    };
    use listing_source_core::Domain;
    use test_api::IntegrationTestService as _;

    #[tokio::test]
    async fn should_not_reclassify_completed_no_pattern_domain_until_reset() {
        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let mut repository = MockListingSourceUrlPatternRepository::new();
        repository
            .expect_find_pattern()
            .times(1)
            .returning(move |_, _| {
                Box::pin(async move {
                    Ok(Some(ListingSourceUrlPatternRecord {
                        listing_source_id,
                        domain_id,
                        listing_source_domain: Domain::try_from("example.com").unwrap(),
                        url_pattern: None,
                        url_pattern_state: UrlPatternState::NoPattern,
                        last_crawled: None,
                    }))
                })
            });
        let service = UrlPatternServiceImpl::new(
            Arc::new(repository),
            UrlPatternAuthoring::Inferred {
                classification_service: Box::new(MockUrlClassificationService::new()),
                review_repository: None,
                apply_directly: false,
            },
        );

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://example.com",
                None,
                &["https://example.com/item/1".to_owned()],
            )
            .await
            .unwrap();

        assert!(pattern.is_none());
    }

    /// Inserts a ListingSource with one `manual.example.com` domain.
    async fn insert_source_with_domain(pool: &sqlx::PgPool) -> (ListingSourceId, CrawlerDomainId) {
        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
             VALUES ($1, 'Manual source', 'manual-source', TRUE, NOW(), NOW())",
        )
        .bind(listing_source_id.as_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
             VALUES ($1, $2, 'manual.example.com', 'manual.example.com')",
        )
        .bind(domain_id.as_uuid())
        .bind(listing_source_id.as_uuid())
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert domain: {error}"));
        (listing_source_id, domain_id)
    }

    fn auto_service(
        pool: &sqlx::PgPool,
        review_repository: &CrawlerReviewRepository,
        classification_service: MockUrlClassificationService,
    ) -> UrlPatternServiceImpl {
        UrlPatternServiceImpl::new(
            Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
            UrlPatternAuthoring::Inferred {
                classification_service: Box::new(classification_service),
                review_repository: Some(review_repository.clone()),
                apply_directly: true,
            },
        )
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_apply_inferred_pattern_through_approved_review_when_authoring_is_auto() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let mut classification = MockUrlClassificationService::new();
        classification
            .expect_find_product_url_pattern()
            .once()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new("/item/").unwrap())) }));
        let service = auto_service(&pool, &review_repository, classification);
        let urls = ["https://manual.example.com/item/1".to_owned()];

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &urls,
            )
            .await
            .unwrap_or_else(|error| panic!("auto authoring must apply the pattern: {error}"));

        assert_eq!(pattern.as_ref().map(Regex::as_str), Some("/item/"));
        let (live_pattern, review_status): (Option<String>, String) = sqlx::query_as(
            "SELECT d.url_pattern, r.status FROM listing_source_domains d \
             JOIN crawler_reviews r ON r.domain_id = d.domain_id \
             WHERE d.domain_id = $1 AND r.artifact_type = 'URL_PATTERN'",
        )
        .bind(domain_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read applied pattern and its review: {error}"));
        assert_eq!(live_pattern.as_deref(), Some("/item/"));
        assert_eq!(review_status, "APPROVED");
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_not_infer_pattern_while_a_review_is_pending_when_authoring_is_auto() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let urls = ["https://manual.example.com/item/1".to_owned()];
        let pending = review_repository
            .create_url_pattern_review(
                &listing_source_id,
                &domain_id,
                MANUAL_URL_PATTERN_REASON,
                UrlPatternDecision::Undecided,
                &urls,
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("open operator review: {error}"));
        let mut classification = MockUrlClassificationService::new();
        classification.expect_find_product_url_pattern().never();
        let service = auto_service(&pool, &review_repository, classification);

        let result = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &urls,
            )
            .await;

        assert!(matches!(
            result,
            Err(UrlPatternServiceError::PendingReview { review_id, .. }) if review_id == pending
        ));
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_open_undecided_review_without_llm_call_when_authoring_is_manual() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let service = UrlPatternServiceImpl::new(
            Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
            UrlPatternAuthoring::Manual {
                review_repository: review_repository.clone(),
            },
        );
        let urls = ["https://manual.example.com/item/1".to_owned()];

        let first = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &urls,
            )
            .await;
        let second = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &urls,
            )
            .await;

        let Err(UrlPatternServiceError::PendingReview { review_id, .. }) = first else {
            panic!("manual authoring must open a pending review");
        };
        assert!(matches!(
            second,
            Err(UrlPatternServiceError::PendingReview { review_id: pending, .. }) if pending == review_id
        ));
        let review = review_repository
            .get_review(review_id)
            .await
            .unwrap_or_else(|error| panic!("read manual URL pattern review: {error}"));
        assert_eq!(review.review.reason, MANUAL_URL_PATTERN_REASON);
        assert_eq!(
            review.review.candidate_payload["decision"]["type"],
            "UNDECIDED"
        );
        let llm_calls: i64 = sqlx::query_scalar(
            "SELECT llm_calls_count FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read LLM call count: {error}"));
        assert_eq!(llm_calls, 0);
    }

    /// Stores `pattern` as an operator approval would while a crawl is running.
    async fn store_pattern(
        pool: &sqlx::PgPool,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &str,
    ) {
        ListingSourceUrlPatternRepositoryImpl::new(pool.clone())
            .save_pattern(listing_source_id, domain_id, Some(pattern))
            .await
            .unwrap_or_else(|error| panic!("store approved pattern: {error}"));
    }

    /// Returns the URL-pattern review count and LLM call count of the source.
    async fn reviews_and_llm_calls(
        pool: &sqlx::PgPool,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> (i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM crawler_reviews WHERE domain_id = $2 AND artifact_type = 'URL_PATTERN'), \
                    llm_calls_count \
             FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .bind(domain_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("read reviews and LLM calls: {error}"))
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_use_pattern_stored_during_crawl_without_review_when_authoring_is_manual() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        store_pattern(&pool, &listing_source_id, &domain_id, "/item/").await;
        let service = UrlPatternServiceImpl::new(
            Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
            UrlPatternAuthoring::Manual {
                review_repository: CrawlerReviewRepository::new(pool.clone()),
            },
        );

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &["https://manual.example.com/item/1".to_owned()],
            )
            .await
            .unwrap_or_else(|error| panic!("stored pattern must be used: {error}"));

        assert_eq!(pattern.as_ref().map(Regex::as_str), Some("/item/"));
        assert_eq!(
            reviews_and_llm_calls(&pool, &listing_source_id, &domain_id).await,
            (0, 0)
        );
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_use_pattern_stored_during_crawl_without_inference_when_authoring_is_auto() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        store_pattern(&pool, &listing_source_id, &domain_id, "/item/").await;
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let mut classification = MockUrlClassificationService::new();
        classification.expect_find_product_url_pattern().never();
        let service = auto_service(&pool, &review_repository, classification);

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                None,
                &["https://manual.example.com/item/1".to_owned()],
            )
            .await
            .unwrap_or_else(|error| panic!("stored pattern must be used: {error}"));

        assert_eq!(pattern.as_ref().map(Regex::as_str), Some("/item/"));
        assert_eq!(
            reviews_and_llm_calls(&pool, &listing_source_id, &domain_id).await,
            (0, 0)
        );
    }

    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_reclassify_stored_pattern_the_crawl_already_used_when_authoring_is_auto() {
        let pool = test_api::get_postgres_client().await;
        let (listing_source_id, domain_id) = insert_source_with_domain(&pool).await;
        store_pattern(&pool, &listing_source_id, &domain_id, "/product/").await;
        let review_repository = CrawlerReviewRepository::new(pool.clone());
        let mut classification = MockUrlClassificationService::new();
        classification
            .expect_find_product_url_pattern()
            .once()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new("/item/").unwrap())) }));
        let service = auto_service(&pool, &review_repository, classification);
        let failed_pattern = Regex::new("/product/").unwrap();

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://manual.example.com",
                Some(&failed_pattern),
                &["https://manual.example.com/item/1".to_owned()],
            )
            .await
            .unwrap_or_else(|error| panic!("failed pattern must be reclassified: {error}"));

        assert_eq!(pattern.as_ref().map(Regex::as_str), Some("/item/"));
        assert_eq!(
            reviews_and_llm_calls(&pool, &listing_source_id, &domain_id).await,
            (1, 1)
        );
    }
}
