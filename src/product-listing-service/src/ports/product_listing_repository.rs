#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use product_listing_core::product_listing::ProductListing;
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRepositoryError {
    #[error("product current event id did not match expected event id")]
    ProductListingCurrentEventIdConflict,
    #[error("product already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product slug already exists")]
    ProductListingSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductListingLookupByIdFailed,
    #[error("product lookup by source listing identity failed")]
    ProductListingLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
    #[error("product insert failed")]
    ProductListingInsertFailed,
    #[error("product update failed")]
    ProductListingUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductListingSlugPersisted,
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
    #[error("persisted listing availability is invalid")]
    InvalidListingAvailabilityPersisted,
    #[error("persisted listing lifecycle is invalid")]
    InvalidListingLifecyclePersisted,
    #[error("persisted product URL is invalid")]
    InvalidProductListingUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductListingImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductListingImageUrlPersisted,

    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
}

#[async_trait::async_trait]
pub trait ProductListingRepository: Send {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>;

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>;

    async fn insert(
        &mut self,
        product: &ProductListing,
        current_event_id: EventId,
    ) -> Result<Versioned<ProductListing, EventId>, ProductListingRepositoryError>;

    async fn update(
        &mut self,
        product: &ProductListing,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<Versioned<ProductListing, EventId>, ProductListingRepositoryError>;
}

pub trait ProductListingRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingRepository + 'tx;
}
