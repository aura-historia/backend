use crate::{
    ports::{ProductListingStorageVersion, product_listing_delivery::*},
    use_cases::commands::change_product_listing_delivery::{
        ProductListingDeliveryError, capability,
    },
};
use application::{
    error::box_error,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use product_listing_core::{
    delivery::ListingDelivery,
    product_listing_id::{ProductListingId, ProductListingKey},
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDeliveryView {
    pub product_listing_id: ProductListingId,
    pub version: ProductListingStorageVersion,
    pub delivery: Option<ListingDelivery>,
}

#[async_trait::async_trait]
pub trait GetProductListingDeliveryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        key: ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError>;
}

pub struct GetProductListingDeliveryHandler<U, R, V> {
    uow: U,
    reader: R,
    references: V,
}

impl<U, R, V> GetProductListingDeliveryHandler<U, R, V> {
    pub fn new(uow: U, reader: R, references: V) -> Self {
        Self {
            uow,
            reader,
            references,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, V> GetProductListingDeliveryUseCase for GetProductListingDeliveryHandler<U, R, V>
where
    U: UnitOfWork,
    R: ProductListingDeliveryReaderFactory<U::Tx>,
    V: ProductListingDeliveryReferencesFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        key: ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError> {
        capability(context, false)?;
        let mut tx = self
            .uow
            .begin()
            .await
            .map_err(|e| ProductListingDeliveryError::Transaction(box_error(e)))?;
        self.references
            .in_transaction(&mut tx)
            .authorize(context, key.listing_source_id)
            .await?;
        let result = self.reader.in_transaction(&mut tx).get(&key).await?;
        tx.commit()
            .await
            .map_err(|e| ProductListingDeliveryError::Transaction(box_error(e)))?;
        Ok(result)
    }
}
