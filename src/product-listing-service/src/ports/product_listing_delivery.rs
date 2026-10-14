use crate::use_cases::{ProductListingDeliveryError, ProductListingDeliveryView};
use application::operation_context::OperationContext;
use listing_source_core::ListingSourceId;
use product_listing_core::{delivery::ListingDelivery, product_listing_id::ProductListingKey};

/// Current source/grant, integration, profile and location validation, held through commit.
#[async_trait::async_trait]
pub trait ProductListingDeliveryReferences: Send {
    async fn authorize(
        &mut self,
        context: &OperationContext,
        source: ListingSourceId,
    ) -> Result<(), ProductListingDeliveryError>;
    /// Locks the listing before loading it, including commands whose semantic effect is empty.
    async fn lock_target(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<(), ProductListingDeliveryError>;
    async fn validate(
        &mut self,
        delivery: &ListingDelivery,
    ) -> Result<(), ProductListingDeliveryError>;
}
pub trait ProductListingDeliveryReferencesFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ProductListingDeliveryReferences + 'tx;
}
#[async_trait::async_trait]
pub trait ProductListingDeliveryReader: Send {
    async fn get(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError>;
}
pub trait ProductListingDeliveryReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingDeliveryReader + 'tx;
}

/// Composition roots must explicitly wire delivery capability before accepting its commands.
#[derive(Clone, Copy, Default)]
pub struct UnavailableProductListingDeliveryReferences;
impl<Tx: Send> ProductListingDeliveryReferencesFactory<Tx>
    for UnavailableProductListingDeliveryReferences
{
    fn in_transaction<'tx>(
        &'tx self,
        _tx: &'tx mut Tx,
    ) -> impl ProductListingDeliveryReferences + 'tx {
        Self
    }
}
#[async_trait::async_trait]
impl ProductListingDeliveryReferences for UnavailableProductListingDeliveryReferences {
    async fn authorize(
        &mut self,
        _: &OperationContext,
        _: ListingSourceId,
    ) -> Result<(), ProductListingDeliveryError> {
        Err(ProductListingDeliveryError::Unavailable)
    }
    async fn lock_target(
        &mut self,
        _: &ProductListingKey,
    ) -> Result<(), ProductListingDeliveryError> {
        Err(ProductListingDeliveryError::Unavailable)
    }
    async fn validate(&mut self, _: &ListingDelivery) -> Result<(), ProductListingDeliveryError> {
        Err(ProductListingDeliveryError::Unavailable)
    }
}
