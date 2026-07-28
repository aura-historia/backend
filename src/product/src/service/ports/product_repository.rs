#![allow(dead_code)]

use crate::core::product_aggregate::Product;
use common::event_id::EventId;
use common::product_id::{ProductId, ProductKey};
use common::versioned::Versioned;

#[derive(Debug, thiserror::Error)]
pub enum ProductRepositoryError {
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
    #[error("product already exists for shop product key")]
    ProductKeyAlreadyExists,
    #[error("product slug already exists")]
    ProductSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductLookupByIdFailed,
    #[error("product lookup by natural key failed")]
    ProductLookupByKeyFailed,
    #[error("product insert failed")]
    ProductInsertFailed,
    #[error("product update failed")]
    ProductUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductSlugPersisted,
    #[error("persisted title is incomplete")]
    IncompleteTitlePersisted,
    #[error("persisted title language is invalid")]
    InvalidTitleLanguagePersisted,
    #[error("persisted description is incomplete")]
    IncompleteDescriptionPersisted,
    #[error("persisted description language is invalid")]
    InvalidDescriptionLanguagePersisted,
    #[error("persisted price is incomplete")]
    IncompletePricePersisted,
    #[error("persisted price amount is negative")]
    NegativePriceAmountPersisted,
    #[error("persisted price currency is invalid")]
    InvalidPriceCurrencyPersisted,
    #[error("persisted product state is invalid")]
    InvalidProductStatePersisted,
    #[error("persisted product lifecycle is invalid")]
    InvalidProductLifecyclePersisted,
    #[error("persisted product URL is invalid")]
    InvalidProductUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductImageUrlPersisted,
    #[error("persisted product image prohibited-content value is invalid")]
    InvalidProductImageProhibitedContentPersisted,
    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
}

#[async_trait::async_trait]
pub trait ProductRepository {
    async fn find_by_id(
        &mut self,
        id: ProductId,
    ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>;

    async fn find_by_key(
        &mut self,
        key: &ProductKey,
    ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>;

    async fn insert(
        &mut self,
        product: &Product,
        current_event_id: EventId,
    ) -> Result<(), ProductRepositoryError>;

    async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<(), ProductRepositoryError>;
}
