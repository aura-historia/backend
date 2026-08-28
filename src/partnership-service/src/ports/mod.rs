use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_core::{
    partnership::Partnership,
    partnership_application::{PartnershipApplication, PartnershipProposal},
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
    partnership_id::PartnershipId,
};
use party_core::party_id::PartyId;
use user_core::user_id::UserId;

domain_primitives::version_newtype!(PartnershipApplicationStorageVersion);
domain_primitives::version_newtype!(PartnershipStorageVersion);
pub type VersionedPartnershipApplication =
    Versioned<PartnershipApplication, PartnershipApplicationStorageVersion>;
pub type VersionedPartnership = Versioned<Partnership, PartnershipStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum PartnershipRepositoryError {
    #[error("partnership already exists for party")]
    PartyConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary partnership persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait PartnershipRepository: Send {
    async fn find_by_party_id(
        &mut self,
        party_id: PartyId,
    ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError>;
    async fn insert(
        &mut self,
        partnership: &Partnership,
    ) -> Result<VersionedPartnership, PartnershipRepositoryError>;
}
pub trait PartnershipRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipRepository + 'tx;
}

#[derive(Debug, thiserror::Error)]
pub enum PartnershipApplicationRepositoryError {
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
    #[error("temporary partnership application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait PartnershipApplicationRepository: Send {
    async fn find_by_id(
        &mut self,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>;
    async fn find_by_user_and_id(
        &mut self,
        user_id: UserId,
        id: PartnershipApplicationId,
    ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>;
    async fn insert(
        &mut self,
        application: &PartnershipApplication,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>;
    async fn update(
        &mut self,
        application: &PartnershipApplication,
        expected: PartnershipApplicationStorageVersion,
    ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>;
}
pub trait PartnershipApplicationRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnershipApplicationRepository + 'tx;
}

#[derive(Debug, thiserror::Error)]
pub enum PartnershipGrantError {
    #[error("temporary partnership grant failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership grant failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait PartnershipMembershipRepository: Send {
    async fn add_member(
        &mut self,
        user_id: UserId,
        partnership_id: PartnershipId,
    ) -> Result<(), PartnershipGrantError>;
}
pub trait PartnershipMembershipRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnershipMembershipRepository + 'tx;
}
#[async_trait::async_trait]
pub trait ListingSourceGrantRepository: Send {
    async fn grant_source_access(
        &mut self,
        partnership_id: PartnershipId,
        listing_source_id: ListingSourceId,
    ) -> Result<(), PartnershipGrantError>;
}
pub trait ListingSourceGrantRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ListingSourceGrantRepository + 'tx;
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnershipApplicationView {
    pub id: PartnershipApplicationId,
    pub applicant_user_id: UserId,
    pub state: PartnershipApplicationState,
    pub proposal: PartnershipProposal,
}
#[derive(Debug, thiserror::Error)]
pub enum PartnershipApplicationReadError {
    #[error("temporary partnership application read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partnership application read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait PartnershipApplicationReader: Send {
    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError>;
    async fn list_all(
        &mut self,
    ) -> Result<Vec<PartnershipApplicationView>, PartnershipApplicationReadError>;
}
pub trait PartnershipApplicationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnershipApplicationReader + 'tx;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdministeredListingSource {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
}
#[derive(Debug, thiserror::Error)]
pub enum SourceAuthorizationError {
    #[error("temporary listing source authorization failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid listing source authorization data")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source authorization failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}
#[async_trait::async_trait]
pub trait ListingSourceAuthorization: Send + Sync {
    async fn can_write_source(
        &self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, SourceAuthorizationError>;
    async fn list_sources_user_administers(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AdministeredListingSource>, SourceAuthorizationError>;
}

pub use listing_source_service::ports::{ListingSourceRepository, ListingSourceRepositoryFactory};
pub use party_service::ports::{PartyRepository, PartyRepositoryFactory};
