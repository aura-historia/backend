#![allow(dead_code)]

use crate::core::shop_aggregate::Shop;
use common::versioned::Versioned;
use common::{shop_id::ShopId, shop_slug_id::ShopSlugId};

common::version_newtype!(ShopStorageVersion);

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
pub(crate) trait ShopRepository {
    async fn find_by_id(
        &mut self,
        id: ShopId,
    ) -> Result<Option<Versioned<Shop, ShopStorageVersion>>, ShopRepositoryError>;

    async fn find_by_slug(
        &mut self,
        slug_id: &ShopSlugId,
    ) -> Result<Option<Versioned<Shop, ShopStorageVersion>>, ShopRepositoryError>;

    async fn insert(&mut self, shop: &Shop) -> Result<(), ShopRepositoryError>;

    async fn update(
        &mut self,
        shop: &Shop,
        expected_version: ShopStorageVersion,
    ) -> Result<(), ShopRepositoryError>;
}
