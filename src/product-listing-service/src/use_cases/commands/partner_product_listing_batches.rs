//! Ordered synchronous integration batches. Each existing item use case owns its transaction.
use super::{
    create_product_listing::*, update_product_listing::*, upsert_product_listing::*,
    withdraw_product_listing::*,
};
use application::operation_context::{OperationContext, Principal};
use product_listing_core::product_listing_id::ProductListingKey;
use std::{future::Future, sync::Arc};

pub const MAX_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingBatchInputError {
    #[error("authenticated actor required to process product listing batch")]
    AuthenticationRequired,
    #[error("Body cannot contain more than {MAX_PRODUCT_LISTING_BATCH_SIZE} products.")]
    TooManyItems,
}

#[derive(Debug)]
pub struct ProductListingBatchItem<T, E> {
    pub index: usize,
    pub product_key: ProductListingKey,
    pub result: Result<T, E>,
}

#[derive(Debug)]
pub struct ProductListingBatchResult<T, E> {
    pub items: Vec<ProductListingBatchItem<T, E>>,
}

pub type CreateProductListingsResult =
    ProductListingBatchResult<CreateProductListingResult, CreateProductListingError>;
pub type UpdateProductListingsResult =
    ProductListingBatchResult<UpdateProductListingResult, UpdateProductListingError>;
pub type UpsertProductListingsResult =
    ProductListingBatchResult<UpsertProductListingResult, UpsertProductListingError>;
pub type WithdrawProductListingsResult =
    ProductListingBatchResult<WithdrawProductListingResult, WithdrawProductListingError>;

#[async_trait::async_trait]
pub trait PartnerProductListingBatchesUseCase: Send + Sync {
    async fn create(
        &self,
        context: &OperationContext,
        commands: Vec<CreateProductListingCommand>,
    ) -> Result<CreateProductListingsResult, ProductListingBatchInputError>;
    async fn update(
        &self,
        context: &OperationContext,
        commands: Vec<(ProductListingKey, UpdateProductListingCommand)>,
    ) -> Result<UpdateProductListingsResult, ProductListingBatchInputError>;
    async fn upsert(
        &self,
        context: &OperationContext,
        commands: Vec<UpsertProductListingCommand>,
    ) -> Result<UpsertProductListingsResult, ProductListingBatchInputError>;
    async fn withdraw(
        &self,
        context: &OperationContext,
        keys: Vec<ProductListingKey>,
    ) -> Result<WithdrawProductListingsResult, ProductListingBatchInputError>;
}

pub struct PartnerProductListingBatchesHandler {
    create: Arc<dyn CreateProductListingUseCase>,
    update: Arc<dyn UpdateProductListingUseCase>,
    upsert: Arc<dyn UpsertProductListingUseCase>,
    withdraw: Arc<dyn WithdrawProductListingUseCase>,
}

impl PartnerProductListingBatchesHandler {
    pub fn new(
        create: Arc<dyn CreateProductListingUseCase>,
        update: Arc<dyn UpdateProductListingUseCase>,
        upsert: Arc<dyn UpsertProductListingUseCase>,
        withdraw: Arc<dyn WithdrawProductListingUseCase>,
    ) -> Self {
        Self {
            create,
            update,
            upsert,
            withdraw,
        }
    }
}

#[async_trait::async_trait]
impl PartnerProductListingBatchesUseCase for PartnerProductListingBatchesHandler {
    async fn create(
        &self,
        context: &OperationContext,
        commands: Vec<CreateProductListingCommand>,
    ) -> Result<CreateProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |command| {
                ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone())
            },
            |command| self.create.execute(context, command),
        )
        .await
    }
    async fn update(
        &self,
        context: &OperationContext,
        commands: Vec<(ProductListingKey, UpdateProductListingCommand)>,
    ) -> Result<UpdateProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |(key, _)| key.clone(),
            |(key, command)| self.update.execute_by_key(context, key, command),
        )
        .await
    }
    async fn upsert(
        &self,
        context: &OperationContext,
        commands: Vec<UpsertProductListingCommand>,
    ) -> Result<UpsertProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |command| {
                ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone())
            },
            |command| self.upsert.execute(context, command),
        )
        .await
    }
    async fn withdraw(
        &self,
        context: &OperationContext,
        keys: Vec<ProductListingKey>,
    ) -> Result<WithdrawProductListingsResult, ProductListingBatchInputError> {
        execute_batch(context, keys, Clone::clone, |key| {
            self.withdraw.execute_by_key(context, key)
        })
        .await
    }
}

async fn execute_batch<C, T, E, F: Future<Output = Result<T, E>>>(
    context: &OperationContext,
    commands: Vec<C>,
    key: impl Fn(&C) -> ProductListingKey,
    execute: impl Fn(C) -> F,
) -> Result<ProductListingBatchResult<T, E>, ProductListingBatchInputError> {
    if matches!(context.principal, Principal::Anonymous) {
        return Err(ProductListingBatchInputError::AuthenticationRequired);
    }
    if commands.len() > MAX_PRODUCT_LISTING_BATCH_SIZE {
        return Err(ProductListingBatchInputError::TooManyItems);
    }
    let mut items = Vec::with_capacity(commands.len());
    // Await every item before the next, including duplicate keys. Failures never undo prior commits.
    for (index, command) in commands.into_iter().enumerate() {
        let product_key = key(&command);
        items.push(ProductListingBatchItem {
            index,
            product_key,
            result: execute(command).await,
        });
    }
    Ok(ProductListingBatchResult { items })
}

#[cfg(test)]
mod tests;
