#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use product_listing_core::product::Product;
use product_listing_core::product_id::{ProductId, ProductKey};

#[derive(Debug, thiserror::Error)]
pub enum ProductRepositoryError {
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
    #[error("product already exists for shop product identity")]
    ShopProductAlreadyExists,
    #[error("product slug already exists")]
    ProductSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductLookupByIdFailed,
    #[error("product lookup by shop product identity failed")]
    ProductLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
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
pub trait ProductRepository: Send {
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
    ) -> Result<Versioned<Product, EventId>, ProductRepositoryError>;

    async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<Versioned<Product, EventId>, ProductRepositoryError>;
}

pub trait ProductRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductRepository + 'tx;
}
