#![allow(dead_code)]

use common::error::boxed::BoxError;
use common::versioned::Versioned;
use common::{shop_id::ShopId, shop_slug_id::ShopSlugId};
use shop_core::shop::Shop;

common::version_newtype!(ShopStorageVersion);

pub type VersionedShop = Versioned<Shop, ShopStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum ShopRepositoryError {
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("shop slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted shop state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
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

    async fn insert(&mut self, shop: &Shop) -> Result<(), ShopRepositoryError>;

    async fn update(
        &mut self,
        shop: &Shop,
        expected_version: ShopStorageVersion,
    ) -> Result<(), ShopRepositoryError>;
}

pub trait ShopRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ShopRepository + 'tx;
}
