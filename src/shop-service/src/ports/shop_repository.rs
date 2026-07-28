#![allow(dead_code)]

use common::versioned::Versioned;
use common::{shop_id::ShopId, shop_slug_id::ShopSlugId, write_metadata::WriteMetadata};
use shop_core::shop::Shop;

common::version_newtype!(ShopStorageVersion);

pub type VersionedShop = Versioned<Shop, ShopStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum ShopRepositoryError {
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("shop slug conflict")]
    SlugConflict,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted shop state")]
    InvalidPersistedState,
    #[error("internal persistence failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait ShopRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ShopId,
    ) -> Result<Option<VersionedShop>, ShopRepositoryError>;

    async fn find_by_slug(
        &mut self,
        slug_id: &ShopSlugId,
    ) -> Result<Option<VersionedShop>, ShopRepositoryError>;

    async fn insert(
        &mut self,
        shop: &Shop,
        metadata: &WriteMetadata,
    ) -> Result<(), ShopRepositoryError>;

    async fn update(
        &mut self,
        shop: &Shop,
        expected_version: ShopStorageVersion,
        metadata: &WriteMetadata,
    ) -> Result<(), ShopRepositoryError>;
}

pub trait ShopRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShopRepository + 'tx;
}
