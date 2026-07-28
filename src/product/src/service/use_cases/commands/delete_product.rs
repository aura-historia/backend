use crate::service::ports::product_event_store::ProductEventStoreError;
use crate::service::ports::product_repository::ProductRepositoryError;
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::product_id::ProductId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductCommand {
    pub product_id: ProductId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductResult {
    pub product_id: ProductId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductError {
    #[error("authenticated actor required to delete product")]
    AuthenticatedActorRequired,
    #[error("product not found")]
    ProductNotFound,
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
    #[error("product event already exists")]
    ProductEventAlreadyExists,
    #[error("product event append failed")]
    ProductEventAppendFailed,
    #[error("current product event lookup failed")]
    CurrentProductEventLookupFailed,
    #[error("failed to begin delete product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteProductCommand,
    ) -> Result<DeleteProductResult, DeleteProductError>;
}

impl From<ProductRepositoryError> for DeleteProductError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ProductCurrentEventIdConflict => {
                Self::ProductCurrentEventIdConflict
            }
            ProductRepositoryError::ProductKeyAlreadyExists => Self::ProductKeyAlreadyExists,
            ProductRepositoryError::ProductSlugAlreadyExists => Self::ProductSlugAlreadyExists,
            ProductRepositoryError::ProductLookupByIdFailed => Self::ProductLookupByIdFailed,
            ProductRepositoryError::ProductLookupByKeyFailed => Self::ProductLookupByKeyFailed,
            ProductRepositoryError::ProductInsertFailed => Self::ProductInsertFailed,
            ProductRepositoryError::ProductUpdateFailed => Self::ProductUpdateFailed,
            ProductRepositoryError::InvalidProductSlugPersisted => {
                Self::InvalidProductSlugPersisted
            }
            ProductRepositoryError::IncompleteTitlePersisted => Self::IncompleteTitlePersisted,
            ProductRepositoryError::InvalidTitleLanguagePersisted => {
                Self::InvalidTitleLanguagePersisted
            }
            ProductRepositoryError::IncompleteDescriptionPersisted => {
                Self::IncompleteDescriptionPersisted
            }
            ProductRepositoryError::InvalidDescriptionLanguagePersisted => {
                Self::InvalidDescriptionLanguagePersisted
            }
            ProductRepositoryError::IncompletePricePersisted => Self::IncompletePricePersisted,
            ProductRepositoryError::NegativePriceAmountPersisted => {
                Self::NegativePriceAmountPersisted
            }
            ProductRepositoryError::InvalidPriceCurrencyPersisted => {
                Self::InvalidPriceCurrencyPersisted
            }
            ProductRepositoryError::InvalidProductStatePersisted => {
                Self::InvalidProductStatePersisted
            }
            ProductRepositoryError::InvalidProductLifecyclePersisted => {
                Self::InvalidProductLifecyclePersisted
            }
            ProductRepositoryError::InvalidProductUrlPersisted => Self::InvalidProductUrlPersisted,
            ProductRepositoryError::InvalidProductImagesPersisted => {
                Self::InvalidProductImagesPersisted
            }
            ProductRepositoryError::InvalidProductImageUrlPersisted => {
                Self::InvalidProductImageUrlPersisted
            }
            ProductRepositoryError::InvalidProductImageProhibitedContentPersisted => {
                Self::InvalidProductImageProhibitedContentPersisted
            }
            ProductRepositoryError::InvalidAggregateStatePersisted => {
                Self::InvalidAggregateStatePersisted
            }
        }
    }
}

impl From<ProductEventStoreError> for DeleteProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::ProductEventAlreadyExists => Self::ProductEventAlreadyExists,
            ProductEventStoreError::ProductEventAppendFailed => Self::ProductEventAppendFailed,
            ProductEventStoreError::CurrentProductEventLookupFailed => {
                Self::CurrentProductEventLookupFailed
            }
        }
    }
}
