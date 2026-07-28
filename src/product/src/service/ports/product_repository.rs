#![allow(dead_code)]

use crate::core::product_aggregate::Product;
use common::event_id::EventId;
use common::product_id::{ProductId, ProductKey};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LoadedProduct {
    pub product: Product,
    pub current_event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductRepositoryError {
    #[error("concurrent product update")]
    ConcurrencyConflict,
    #[error("product key conflict")]
    ProductKeyConflict,
    #[error("product slug conflict")]
    SlugConflict,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted product state")]
    InvalidPersistedState,
    #[error("internal persistence failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ProductRepository {
    async fn find_by_id(
        &mut self,
        id: ProductId,
    ) -> Result<Option<LoadedProduct>, ProductRepositoryError>;

    async fn find_by_key(
        &mut self,
        key: &ProductKey,
    ) -> Result<Option<LoadedProduct>, ProductRepositoryError>;

    async fn insert(
        &mut self,
        product: &Product,
        current_event_id: EventId,
        created_by: &str,
    ) -> Result<(), ProductRepositoryError>;

    async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
        updated_by: &str,
    ) -> Result<(), ProductRepositoryError>;
}
