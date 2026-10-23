//! Source-keyed queue admission and non-secret Shopify configuration.
mod error;
#[cfg(test)]
mod tests;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    listing_sources::shopify::PutShopifyConfigurationData,
    partner_product_listings::{
        CreateProductListingData, PartnerProductListingInput, UpdateProductListingData,
        UpsertProductListingData, WithdrawProductListingData,
        admission::{self, AdmissionReportData, RawAdmissionItems},
    },
    product_listing_management::MutationInputError,
};
use aura_historia_runtime::{PartnerProductListingIngestion, with_publication_deadline};
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::{
    ProductListingIngestionIntent, ProductListingIngestionSubmission,
    SubmitPartnerProductListingIngestionUseCase,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};

const RESPONSE_RESERVE: Duration = Duration::from_millis(500);
const ADMISSION_DESCRIPTION: &str = "Submit up to 100 source-keyed listings for FIFO queue admission. Invalid items are rejected independently and original positions are retained. PUBLISHED confirms queue custody; source grants and business rules are checked when the worker applies each command. REJECTED, NOT_ATTEMPTED and UNKNOWN are distinct; admission never confirms a committed listing mutation. submissionId is correlation, not a status resource. Optional idempotencyKey accepts 1-128 visible ASCII bytes excluding comma; omission generates a key returned in the report, unrecoverable after a lost reply. Retry a partial or lost reply only with the same actor, source, effective key and complete unchanged ordered batch, retaining content, operations, original indices and originalInputCount. No automatic retries or exactly-once guarantee.";

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, bound(deserialize = ""))]
struct BatchInput<T> {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "admission::optional_idempotency_key_schema")]
    idempotency_key: Option<String>,
    products: RawAdmissionItems<T>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShopifyInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    configuration: PutShopifyConfigurationData,
}
#[derive(Serialize, JsonSchema)]
struct ShopifyResult {
    created: bool,
}
fn policy(
    capability: CredentialCapability,
    destructive: bool,
    idempotent: bool,
) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![capability],
        read_only: false,
        destructive,
        idempotent,
        open_world: false,
    }
}

/// Family only; partner release composition publishes the combined catalogue.
pub fn operations(
    services: PartnerProductListingIngestion,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    let mut operations = vec![
        batch_operation(
            services.submit.clone(),
            "postAsyncPartnerProductListings",
            false,
            |data: CreateProductListingData, source| {
                data.into_command(source)
                    .map(ProductListingIngestionIntent::Create)
            },
        )?,
        batch_operation(
            services.submit.clone(),
            "patchAsyncPartnerProductListings",
            true,
            |data: UpdateProductListingData, source| {
                data.into_key_and_command(source)
                    .map(
                        |(product_key, command)| ProductListingIngestionIntent::Update {
                            product_key,
                            command,
                        },
                    )
            },
        )?,
        batch_operation(
            services.submit.clone(),
            "putAsyncPartnerProductListings",
            true,
            |data: UpsertProductListingData, source| {
                data.into_command(source)
                    .map(ProductListingIngestionIntent::Upsert)
            },
        )?,
        batch_operation(
            services.submit,
            "deleteAsyncPartnerProductListings",
            true,
            |data: WithdrawProductListingData, source| {
                data.into_product_key(source)
                    .map(ProductListingIngestionIntent::Withdraw)
            },
        )?,
    ];
    operations.push(OperationDefinition::new(
        "putShopifyListingSourceIngestionConfiguration",
        "Replace the complete non-secret Shopify domain, optional currency and language configuration. Missing or null currency/language clears the provider override. Uses current source grants and listing-sources:write; created reports whether Shopify was newly configured. This commits source configuration synchronously.",
        policy(CredentialCapability::ListingSourcesWrite, true, true),
        move |input: ShopifyInput, context| {
            let service = services.put_configuration.clone();
            async move {
                let configuration = input.configuration.into_configuration().map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context.execute(service.execute(&context.operation, partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::PutListingSourceIngestionConfigurationCommand { listing_source_id: input.listing_source_id, configuration })).await?.map_err(error::configuration)?;
                Ok(ShopifyResult { created: result.created })
            }
        },
    )?);
    Ok(operations)
}

fn batch_operation<T, F>(
    service: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
    name: &'static str,
    destructive: bool,
    into_intent: F,
) -> Result<OperationDefinition, RegistryError>
where
    T: PartnerProductListingInput + DeserializeOwned + JsonSchema + Send + 'static,
    F: Fn(T, ListingSourceId) -> Result<ProductListingIngestionIntent, MutationInputError>
        + Send
        + Sync
        + 'static,
{
    let into_intent = Arc::new(into_intent);
    OperationDefinition::new(
        name,
        ADMISSION_DESCRIPTION,
        policy(
            CredentialCapability::ProductListingsWrite,
            destructive,
            false,
        ),
        move |input: BatchInput<T>, context| {
            let service = service.clone();
            let into_intent = into_intent.clone();
            async move {
                let key = input
                    .idempotency_key
                    .as_deref()
                    .map(admission::parse_idempotency_key)
                    .transpose()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let raw = input.products.into_raw();
                let count = raw.len();
                // DTOs remain the sole item decoder. Only their raw representation differs from sync batches.
                let (items, source_ids, failures) = admission::parse_items::<T>(
                    raw,
                    |data| data.source_listing_id(),
                    |data| into_intent(data, input.listing_source_id),
                );
                let deadline = context
                    .deadline
                    .checked_sub(RESPONSE_RESERVE)
                    .unwrap_or_else(tokio::time::Instant::now);
                let result = context
                    .execute(with_publication_deadline(
                        deadline,
                        service.execute(
                            &context.operation,
                            ProductListingIngestionSubmission {
                                listing_source_id: input.listing_source_id,
                                original_input_count: count,
                                idempotency_key: key,
                                items,
                            },
                        ),
                    ))
                    .await?
                    .map_err(error::submission)?;
                Ok::<AdmissionReportData, OperationError>(admission::report(
                    result,
                    failures,
                    &source_ids,
                ))
            }
        },
    )
}
