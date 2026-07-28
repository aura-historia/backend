use crate::core::product_aggregate::{ProductAddress, ProductAuction, ProductPricing};
use crate::core::product_image::ProductImage;
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
    pub embedding: PatchField<Vec<f32>>,
    pub auction: PatchField<ProductAuction>,
}

impl UpdateProductCommand {
    pub fn is_empty(&self) -> bool {
        !self.address.is_changed()
            && !self.pricing.is_changed()
            && !self.state.is_changed()
            && !self.url.is_changed()
            && !self.images.is_changed()
            && !self.embedding.is_changed()
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
    #[error("product not found")]
    NotFound,
    #[error("concurrent product update")]
    ConcurrencyConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid product update")]
    InvalidProduct,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait UpdateProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError>;
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
