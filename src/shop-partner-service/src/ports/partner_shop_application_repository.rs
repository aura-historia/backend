#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use shop_partner_core::partner_shop_application::PartnerShopApplication;
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use user_core::user_id::UserId;

domain_primitives::version_newtype!(PartnerShopApplicationStorageVersion);

pub type VersionedPartnerShopApplication =
    Versioned<PartnerShopApplication, PartnerShopApplicationStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopApplicationRepositoryError {
    #[error("concurrent partner shop application update")]
    ConcurrencyConflict,
    #[error("temporary partner shop application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partner shop application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnerShopApplicationRepository: Send {
    async fn find_by_user_and_id(
        &mut self,
        user_id: UserId,
        id: PartnerShopApplicationId,
    ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>;

    async fn find_by_id(
        &mut self,
        id: PartnerShopApplicationId,
    ) -> Result<Option<VersionedPartnerShopApplication>, PartnerShopApplicationRepositoryError>;

    async fn insert(
        &mut self,
        application: &PartnerShopApplication,
    ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>;

    async fn update(
        &mut self,
        application: &PartnerShopApplication,
        expected_version: PartnerShopApplicationStorageVersion,
    ) -> Result<VersionedPartnerShopApplication, PartnerShopApplicationRepositoryError>;

    async fn delete(
        &mut self,
        id: PartnerShopApplicationId,
        expected_version: PartnerShopApplicationStorageVersion,
    ) -> Result<(), PartnerShopApplicationRepositoryError>;
}

pub trait PartnerShopApplicationRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnerShopApplicationRepository + 'tx;
}
