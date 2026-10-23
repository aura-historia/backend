//! Four synchronous integration operations; batch execution belongs to product-listing-service.
mod error;
#[cfg(test)]
mod tests;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::partner_product_listings::{
    CreateProductListingData, MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE,
    ProductListingBatchReportData, UpdateProductListingData, UpsertProductListingData,
    WithdrawProductListingData, prepare_batch,
};
use aura_historia_runtime::PartnerProductListingBatches;
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::commands::partner_product_listing_batches::PartnerProductListingBatchesUseCase;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchInput<T> {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(length(max = MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE))]
    products: Vec<T>,
}
fn policy(destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsWrite],
        read_only: false,
        destructive,
        idempotent: false,
        open_world: false,
    }
}

/// Family only; the partner release owns combined-registry publication.
pub fn operations(
    services: PartnerProductListingBatches,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        create_operation(services.batches.clone())?,
        update_operation(services.batches.clone())?,
        upsert_operation(services.batches.clone())?,
        withdraw_operation(services.batches)?,
    ])
}

fn create_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "postPartnerProductListings",
        "Create source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(false),
        move |input: BatchInput<CreateProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.create(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::create,
                ))
            }
        },
    )
}

fn update_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "patchPartnerProductListings",
        "Patch existing source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<UpdateProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.update(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::update,
                ))
            }
        },
    )
}

fn upsert_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "putPartnerProductListings",
        "Create, update or restore source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<UpsertProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.upsert(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::upsert,
                ))
            }
        },
    )
}

fn withdraw_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deletePartnerProductListings",
        "Withdraw and retain source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<WithdrawProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.withdraw(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::withdraw,
                ))
            }
        },
    )
}
