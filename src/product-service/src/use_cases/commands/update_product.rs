use crate::ports::product_event_store::ProductEventStoreError;
use crate::ports::product_repository::ProductRepositoryError;
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use indexmap::IndexSet;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use url::Url;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductCommand {
    pub product_id: ProductId,
    pub address: PatchField<ProductAddress>,
    pub pricing: PatchField<ProductPricing>,
    pub state: PatchField<ProductState>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductImage>>,
    pub auction: PatchField<ProductAuction>,
}

impl UpdateProductCommand {
    pub fn is_empty(&self) -> bool {
        !self.address.is_changed()
            && !self.pricing.is_changed()
            && !self.state.is_changed()
            && !self.url.is_changed()
            && !self.images.is_changed()
            && !self.auction.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductResult {
    pub product_id: ProductId,
    pub event_id: Option<EventId>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductError {
    #[error("authenticated actor required to update product")]
    AuthenticatedActorRequired,
    #[error("product not found")]
    ProductNotFound,
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
    #[error("product update cleared required state")]
    StateRequired,
    #[error("product update cleared required url")]
    UrlRequired,
    #[error("product state is invalid")]
    InvalidProductState,
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
    #[error("failed to begin update product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError>;
}

impl From<ProductRepositoryError> for UpdateProductError {
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

impl From<ProductEventStoreError> for UpdateProductError {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateProductCommand {
            product_id: ProductId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }
}
