#![allow(dead_code)]

use crate::core::{shop_aggregate::Shop, shop_version::ShopVersion};
use common::{shop_id::ShopId, shop_slug_id::ShopSlugId};

#[derive(Debug, thiserror::Error)]
pub enum ShopRepositoryError {
    #[error("shop version conflict")]
    VersionConflict,
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
    async fn find_by_id(&mut self, id: ShopId) -> Result<Option<Shop>, ShopRepositoryError>;

    async fn find_by_slug(
        &mut self,
        slug_id: &ShopSlugId,
    ) -> Result<Option<Shop>, ShopRepositoryError>;

    async fn insert(&mut self, shop: &Shop) -> Result<(), ShopRepositoryError>;

    async fn update(
        &mut self,
        shop: &Shop,
        expected_version: ShopVersion,
    ) -> Result<ShopVersion, ShopRepositoryError>;
}
