use application::error::BoxError;
use application::operation_context::Principal;
use application::patch_field::PatchField;
use listing_source_core::{
    AcquisitionMethod, Domain, ListingSource, ListingSourceId, ListingSourceName,
    ListingSourceSlugId,
};
use localization::Language;
use money::Currency;
use party_core::{party_id::PartyId, party_name::PartyName, party_slug_id::PartySlugId};
use time::OffsetDateTime;

pub use party_service::ports::{PartyRepository, PartyRepositoryFactory};

domain_primitives::version_newtype!(ListingSourceStorageVersion);

#[derive(Debug, Clone, PartialEq)]
pub enum AcquisitionConfiguration {
    WebCrawl,
    Shopify {
        domain: Domain,
        currency: Option<Currency>,
        language: Option<Language>,
    },
    Woocommerce {
        currency: Option<Currency>,
        language: Option<Language>,
    },
    PartnerApi,
}

impl AcquisitionConfiguration {
    pub fn method(&self) -> AcquisitionMethod {
        match self {
            Self::WebCrawl => AcquisitionMethod::WebCrawl,
            Self::Shopify { .. } => AcquisitionMethod::Shopify,
            Self::Woocommerce { .. } => AcquisitionMethod::Woocommerce,
            Self::PartnerApi => AcquisitionMethod::PartnerApi,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListingSourceAcquisitionConfigurations(pub Vec<AcquisitionConfiguration>);

impl ListingSourceAcquisitionConfigurations {
    pub fn validate_for(
        &self,
        source: &ListingSource,
    ) -> Result<(), AcquisitionConfigurationMismatch> {
        let configured = self
            .0
            .iter()
            .map(AcquisitionConfiguration::method)
            .collect::<std::collections::HashSet<_>>();
        if configured.len() == self.0.len() && configured == *source.acquisition_methods() {
            Ok(())
        } else {
            Err(AcquisitionConfigurationMismatch)
        }
    }

    pub fn has_woocommerce(&self) -> bool {
        self.0.iter().any(|configuration| {
            matches!(configuration, AcquisitionConfiguration::Woocommerce { .. })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("acquisition methods and configuration do not match")]
pub struct AcquisitionConfigurationMismatch;

#[derive(Debug, Clone, PartialEq)]
pub struct StoredListingSource {
    pub source: ListingSource,
    pub configuration: ListingSourceAcquisitionConfigurations,
    pub version: ListingSourceStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceRepositoryError {
    #[error("concurrent listing source update")]
    ConcurrencyConflict,
    #[error("listing source slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("listing source Shopify domain conflict")]
    ShopifyDomainConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary listing source persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError>;
    async fn find_by_slug(
        &mut self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError>;
    async fn insert(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceAcquisitionConfigurations,
        woocommerce_webhook_secret: Option<&str>,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError>;
    async fn update(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceAcquisitionConfigurations,
        woocommerce_webhook_secret: PatchField<&str>,
        expected: ListingSourceStorageVersion,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError>;
}

pub trait ListingSourceRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceRepository + 'tx;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListingSourceDetails {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator_party_id: PartyId,
    pub operator_slug_id: PartySlugId,
    pub operator_name: PartyName,
    pub acquisition_methods: std::collections::HashSet<AcquisitionMethod>,
    pub url: Option<url::Url>,
    pub image: Option<url::Url>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceReadError {
    #[error("temporary listing source read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListingSourceDetailsReader: Send + Sync {
    async fn find_details_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError>;
    async fn find_details_by_slug(
        &self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<ListingSourceDetails>, ListingSourceReadError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopifySource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub domain: Domain,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WoocommerceSource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebCrawlSource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub url: url::Url,
}

#[async_trait::async_trait]
pub trait ShopifySourceReader: Send + Sync {
    async fn find_by_domain(
        &self,
        domain: &Domain,
    ) -> Result<Option<ShopifySource>, ListingSourceReadError>;
}

#[async_trait::async_trait]
pub trait WoocommerceSourceReader: Send + Sync {
    async fn find_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<WoocommerceSource>, ListingSourceReadError>;
}

#[async_trait::async_trait]
pub trait WebCrawlSourceReader: Send + Sync {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceSignatureVerification {
    Valid,
    Invalid,
    SecretNotConfigured,
}

#[async_trait::async_trait]
pub trait WoocommerceSignatureVerifier: Send + Sync {
    async fn verify(
        &self,
        id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError>;
}

/// Authorizes a principal against the Party that operates a ListingSource.
///
/// Iteration 2 deliberately has no PostgreSQL implementation: existing grants bind users to
/// legacy Shops, not Parties. Runtime wiring waits for the Party-based grant model.
#[async_trait::async_trait]
pub trait PartnershipGrantPolicy: Send + Sync {
    async fn can_access_source(
        &self,
        principal: &Principal,
        operator_party_id: PartyId,
    ) -> Result<bool, ListingSourceReadError>;
}
