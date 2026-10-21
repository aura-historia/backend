//! Complete public listing history registration, selected by release composition.
#[cfg(test)]
mod tests;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use aura_historia_contracts::product_listings::history::{
    ProductListingHistoryEntryData, ProductListingHistoryQueryData,
};
use aura_historia_runtime::PublicProductListingHistory;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    GetProductListingHistoryError, GetProductListingHistoryRequest, ProductListingHistoryLookup,
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    query: ProductListingHistoryQueryData,
}

/// Register the existing complete-history operation without installing it globally.
pub fn operations(
    services: PublicProductListingHistory,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![OperationDefinition::new(
        "getProductListingHistory",
        "Get the complete public listing history in original event order, with optional per-event historical display currency. Returns every event in one call; oversized results fail explicitly.",
        OperationPolicy {
            access: AccessClass::Public,
            required_scopes: vec![],
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        move |input: HistoryInput, context| {
            let service = services.get.clone();
            async move {
                context
                    .execute(service.execute(
                        &context.operation,
                        GetProductListingHistoryRequest {
                            lookup: ProductListingHistoryLookup::ById(input.product_listing_id),
                            currency: input.query.currency,
                        },
                    ))
                    .await?
                    .map(|entries| {
                        entries
                            .into_iter()
                            .map(ProductListingHistoryEntryData::from)
                            .collect::<Vec<_>>()
                    })
                    .map_err(history_error)
            }
        },
    )?])
}

fn history_error(error: GetProductListingHistoryError) -> OperationError {
    use GetProductListingHistoryError::*;
    match error {
        NotFound => OperationError::NotFound,
        error @ (QueryFailed { .. }
        | FxSnapshotUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ (InvalidReadModel { .. } | PriceConversionFailed(_)) => {
            OperationError::Internal(Box::new(error))
        }
    }
}
