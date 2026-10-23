//! Source-local delivery inspection, versioned changes and queue admission.
mod error;
#[cfg(test)]
mod tests;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    partner_product_listings::admission::{self, AdmissionReportData},
    product_listing_delivery::{self as contract, CommandData, DeliveryData, MutationData},
};
use aura_historia_runtime::{ProductListingDelivery, with_publication_deadline};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use product_listing_service::use_cases::{
    ChangeProductListingDeliveryUseCase, GetProductListingDeliveryUseCase,
    IndexedProductListingIngestionIntent, ProductListingIngestionIntent,
    ProductListingIngestionSubmission, SubmitPartnerProductListingIngestionUseCase,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "contract::source_listing_id_schema")]
    source_listing_id: SourceListingId,
}
impl ListingInput {
    fn into_key(self) -> ProductListingKey {
        ProductListingKey::new(self.listing_source_id, self.source_listing_id)
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangeInput {
    #[serde(flatten)]
    listing: ListingInput,
    #[serde(flatten)]
    command: CommandData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitInput {
    #[serde(flatten)]
    listing: ListingInput,
    #[serde(flatten)]
    command: CommandData,
    #[serde(default)]
    #[schemars(schema_with = "admission::optional_idempotency_key_schema")]
    idempotency_key: Option<String>,
}
fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn change_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsWrite],
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    }
}
/// Module-local family; the partner release composes the final catalogue.
pub fn operations(
    services: ProductListingDelivery,
    submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        get(services.get)?,
        change(services.change)?,
        submit_operation(submit)?,
    ])
}
fn get(
    service: Arc<dyn GetProductListingDeliveryUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListingDelivery",
        "Read the complete current protected delivery configuration and expectedVersion. Identify the listing by listingSourceId plus opaque sourceListingId, not the internal productListingId used by management tools. Current source authority and product-listings:read are checked by the use case. A null delivery means source-authored default inheritance.",
        read_policy(),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                let view = context
                    .execute(service.execute(&context.operation, input.into_key()))
                    .await?
                    .map_err(error::delivery)?;
                DeliveryData::try_from(view).map_err(OperationError::Internal)
            }
        },
    )
}
fn change(
    service: Arc<dyn ChangeProductListingDeliveryUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "changeProductListingDelivery",
        "Change delivery only on an existing active listing using listingSourceId, opaque sourceListingId and current positive expectedVersion. Preserve every explicit UNCHANGED/CLEAR/SET patch, integration, selection, rate input, whole-component override, terms reference and optional import provenance. Returns CHANGED or UNCHANGED and the current version. Even semantic no-ops validate current source authority and dependencies. Ordinary stale versions conflict; exact committed import retries retain the existing replay behavior and return current state after authority checks. After an ambiguous response refetch and reconcile, retaining exact import keys/content for an import retry. No automatic retries.",
        change_policy(),
        move |input: ChangeInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .command
                    .into_command(input.listing.into_key())
                    .map_err(OperationError::InvalidArguments)?;
                let result = context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map_err(error::delivery)?;
                Ok(MutationData::from(result))
            }
        },
    )
}
fn submit_operation(
    submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "submitProductListingDelivery",
        "Submit one revision-checked delivery command for FIFO queue admission by listingSourceId and opaque sourceListingId. The report retains originalInputCount 1, original index 0 and the full normalized source-local ID. PUBLISHED confirms queue custody only; source grants, current expectedVersion and business rules are checked when the worker applies it. REJECTED, NOT_ATTEMPTED and UNKNOWN remain distinct; admission never confirms a committed mutation. Optional idempotencyKey is 1-128 visible ASCII bytes excluding comma; omission generates a returned key that cannot be recovered after a lost response. Retry a partial or lost response only with the same actor/source, effective key and unchanged command, including expectedVersion/import fields and original index/count. submissionId is correlation, not a status resource. No automatic retries or exactly-once guarantee.",
        OperationPolicy {
            idempotent: false,
            ..change_policy()
        },
        move |input: SubmitInput, context| {
            let service = submit.clone();
            async move {
                let command = input
                    .command
                    .into_command(input.listing.into_key())
                    .map_err(OperationError::InvalidArguments)?;
                let key = input
                    .idempotency_key
                    .as_deref()
                    .map(admission::parse_idempotency_key)
                    .transpose()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let source_ids = [Some(command.product_key.source_listing_id.to_string())];
                let submission = ProductListingIngestionSubmission {
                    listing_source_id: command.product_key.listing_source_id,
                    original_input_count: 1,
                    idempotency_key: key,
                    items: vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: ProductListingIngestionIntent::ChangeDelivery(command),
                    }],
                };
                let deadline = context
                    .deadline
                    .checked_sub(Duration::from_millis(500))
                    .unwrap_or_else(tokio::time::Instant::now);
                let result = context
                    .execute(with_publication_deadline(
                        deadline,
                        service.execute(&context.operation, submission),
                    ))
                    .await?
                    .map_err(super::partner_product_listing_ingestion::error::submission)?;
                Ok::<AdmissionReportData, OperationError>(admission::report(
                    result,
                    vec![],
                    &source_ids,
                ))
            }
        },
    )
}
