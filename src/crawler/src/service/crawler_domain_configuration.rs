//! Explicit crawler-local domain ownership configuration.

use crate::CrawlerDomainId;
use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId};

#[derive(Debug, Clone)]
pub struct CrawlerDomainConfiguration {
    pub domain_id: CrawlerDomainId,
    pub listing_source_id: ListingSourceId,
    pub domain: Domain,
    pub created: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CrawlerDomainRemoval {
    pub domain_id: CrawlerDomainId,
    pub removed_url_count: i64,
    pub removed_url_pattern_review_count: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerDomainConfigurationError {
    #[error("crawler-local ListingSource does not exist")]
    ListingSourceNotFound { listing_source_id: ListingSourceId },
    #[error("crawler domain is already owned by another ListingSource")]
    DomainOwnedByAnotherListingSource {
        domain: Domain,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("crawler domain does not belong to ListingSource")]
    DomainNotOwnedByListingSource {
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    },
    #[error("crawler domain must be a DNS name, not an IP literal")]
    UnsafeDomain { domain: Domain },
    #[error("crawler domain may contain at most one leading www.")]
    RepeatedWwwPrefix { domain: Domain },
    #[error("crawler domain configuration database failure")]
    Database {
        #[source]
        source: application::error::BoxError,
    },
}

#[async_trait]
pub trait CrawlerDomainConfigurationRepository: Send + Sync {
    async fn list_for_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError>;

    async fn register(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError>;

    async fn remove(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError>;
}

#[async_trait]
pub trait CrawlerDomainAdministration: Send + Sync {
    async fn list_crawler_domains(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError>;

    async fn register_crawler_domain(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError>;

    async fn remove_crawler_domain(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError>;
}

pub struct CrawlerDomainAdministrationHandler {
    repository: std::sync::Arc<dyn CrawlerDomainConfigurationRepository>,
}

impl CrawlerDomainAdministrationHandler {
    pub fn new(repository: std::sync::Arc<dyn CrawlerDomainConfigurationRepository>) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl CrawlerDomainAdministration for CrawlerDomainAdministrationHandler {
    async fn list_crawler_domains(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError> {
        self.repository.list_for_source(listing_source_id).await
    }

    async fn register_crawler_domain(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError> {
        self.repository.register(listing_source_id, domain).await
    }

    async fn remove_crawler_domain(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError> {
        self.repository.remove(listing_source_id, domain_id).await
    }
}
