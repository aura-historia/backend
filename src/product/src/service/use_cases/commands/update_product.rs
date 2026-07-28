use crate::core::product_aggregate::{ProductAddress, ProductAuction, ProductPricing};
use crate::core::product_image::ProductImage;
use crate::service::ports::product_event_store::ProductEventStoreError;
use crate::service::ports::product_repository::ProductRepositoryError;
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use indexmap::IndexSet;
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
    #[error("product update conflicted with a concurrent event")]
    ConcurrencyConflict,
    #[error("product update cleared required state")]
    StateRequired,
    #[error("product update cleared required url")]
    UrlRequired,
    #[error("product state is invalid")]
    InvalidProductState,
    #[error("product repository unavailable")]
    ProductRepositoryUnavailable,
    #[error("product event store unavailable")]
    ProductEventStoreUnavailable,
    #[error("product event already exists")]
    EventConflict,
    #[error("failed to begin update product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product transaction")]
    CommitTransactionFailed,
    #[error("internal product repository failure")]
    ProductRepositoryInternal,
    #[error("internal product event store failure")]
    ProductEventStoreInternal,
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
            ProductRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductRepositoryError::TemporarilyUnavailable => Self::ProductRepositoryUnavailable,
            ProductRepositoryError::InvalidPersistedState => Self::InvalidProductState,
            ProductRepositoryError::ProductKeyConflict
            | ProductRepositoryError::SlugConflict
            | ProductRepositoryError::Internal => Self::ProductRepositoryInternal,
        }
    }
}

impl From<ProductEventStoreError> for UpdateProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::TemporarilyUnavailable => Self::ProductEventStoreUnavailable,
            ProductEventStoreError::InvalidEvent => Self::InvalidProductState,
            ProductEventStoreError::EventConflict => Self::EventConflict,
            ProductEventStoreError::Internal => Self::ProductEventStoreInternal,
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
