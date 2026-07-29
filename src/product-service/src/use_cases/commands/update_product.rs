use crate::ports::{
    ProductEventStore, ProductEventStoreError, ProductEventStoreFactory, ProductRepository,
    ProductRepositoryError, ProductRepositoryFactory,
};
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::transaction::{Transaction, UnitOfWork};
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

pub struct UpdateProductHandler<U, R, E> {
    unit_of_work: U,
    products: R,
    events: E,
}

impl<U, R, E> UpdateProductHandler<U, R, E> {
    pub fn new(unit_of_work: U, products: R, events: E) -> Self {
        Self {
            unit_of_work,
            products,
            events,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E> UpdateProductUseCase for UpdateProductHandler<U, R, E>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_product",
        skip_all,
        fields(
            product_id = %command.product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductError::BeginTransactionFailed)?;
        let loaded = self
            .products
            .in_transaction(&mut tx)
            .find_by_id(command.product_id)
            .await?
            .ok_or(UpdateProductError::ProductNotFound)?;
        let expected_event_id = loaded.version;
        let mut product = loaded.value;

        apply_command(&mut product, command)?;
        let events = product.take_pending_events();
        let event_id = events.last().map(|event| event.event_id);

        if let Some(new_event_id) = event_id {
            self.products
                .in_transaction(&mut tx)
                .update(&product, expected_event_id, new_event_id)
                .await?;
            for event in &events {
                self.events.in_transaction(&mut tx).append(event).await?;
            }
        }

        tx.commit()
            .await
            .map_err(|_| UpdateProductError::CommitTransactionFailed)?;

        if let Some(event_id) = event_id {
            tracing::info!(
                event = "product.updated",
                actor_type = context.principal.kind(),
                actor_id = %context.principal.label(),
                product_id = %product.id(),
                event_id = %event_id,
                outcome = "success",
            );
        }

        Ok(UpdateProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

fn apply_command(
    product: &mut product_core::product::Product,
    command: UpdateProductCommand,
) -> Result<(), UpdateProductError> {
    match command.address {
        PatchField::Unchanged => {}
        PatchField::Set(address) => {
            product.replace_address(address);
        }
        PatchField::Clear => {
            product.replace_address(Default::default());
        }
    }
    match command.pricing {
        PatchField::Unchanged => {}
        PatchField::Set(pricing) => {
            product.replace_pricing(pricing);
        }
        PatchField::Clear => {
            product.replace_pricing(Default::default());
        }
    }
    match command.state {
        PatchField::Unchanged => {}
        PatchField::Set(state) => {
            product.change_state(state);
        }
        PatchField::Clear => return Err(UpdateProductError::StateRequired),
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(url) => {
            product.change_url(url);
        }
        PatchField::Clear => return Err(UpdateProductError::UrlRequired),
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(images) => {
            product.replace_images(images);
        }
        PatchField::Clear => {
            product.replace_images(Default::default());
        }
    }
    match command.auction {
        PatchField::Unchanged => {}
        PatchField::Set(auction) => {
            product.replace_auction(auction);
        }
        PatchField::Clear => {
            product.replace_auction(Default::default());
        }
    }

    Ok(())
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
