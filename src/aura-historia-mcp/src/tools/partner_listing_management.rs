//! Partner management calls the same authoritative application handles as REST.
mod error;
#[cfg(test)]
mod tests;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    listing_sources::administered::{
        AdministeredListingSourcesPageData, AdministeredListingSourcesQuery,
    },
    product_listing_management::{
        DetailsData, MutationData, SearchQuery, SearchResponse, UpdateData, VersionData, version,
    },
};
use aura_historia_runtime::PartnerListingManagement;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::commands::partner_product_listings::{
    PartnerProductListingLifecycleCommand, UpdatePartnerProductListingCommand,
};
use schemars::JsonSchema;
use serde::Deserialize;

use partnership_service::use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesUseCase;
use product_listing_service::use_cases::{
    commands::partner_product_listings::{
        RestorePartnerProductListingUseCase, UpdatePartnerProductListingUseCase,
        WithdrawPartnerProductListingUseCase,
    },
    queries::partner_product_listings::{
        GetPartnerProductListingUseCase, ListPartnerProductListingsUseCase,
    },
};
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(transform = require_source_schema)]
struct ListInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    query: SearchQuery,
}
fn require_source_schema(schema: &mut schemars::Schema) {
    // The flattened shared query also supports an optional administrator source filter.
    // This envelope's typed, required path source owns that same member for partner calls.
    schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("derived listing input properties")
        .insert(
            "listingSourceId".into(),
            aura_historia_contracts::wire::schema::object_id(ListingSourceId::PREFIX)
                .as_value()
                .clone(),
        );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    #[serde(rename = "listingSourceId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(rename = "productListingId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    data: UpdateData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LifecycleInput {
    #[serde(rename = "listingSourceId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(rename = "productListingId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    data: VersionData,
}
impl LifecycleInput {
    fn into_command(self) -> Result<PartnerProductListingLifecycleCommand, OperationError> {
        Ok(PartnerProductListingLifecycleCommand {
            listing_source_id: self.listing_source_id,
            product_listing_id: self.product_listing_id,
            expected_version: version(self.data.expected_version).map_err(invalid)?,
        })
    }
}
fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> OperationError {
    OperationError::InvalidArguments(Box::new(error))
}
fn policy(scope: CredentialCapability, read_only: bool, destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![scope],
        read_only,
        destructive,
        idempotent: true,
        open_world: false,
    }
}
/// Family only; the partner release owns the final combined registry.
pub fn operations(
    services: PartnerListingManagement,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        sources_operation(services.sources)?,
        list_operation(services.list)?,
        get_operation(services.get)?,
        update_operation(services.update)?,
        withdraw_operation(services.withdraw)?,
        restore_operation(services.restore)?,
    ])
}

fn sources_operation(
    service: Arc<dyn ListAdministeredListingSourcesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getMyListingSources",
        "Page through your currently granted listing sources in stable name and ID order. Defaults to 50, maximum 100; pass next as after. Grants and delegated capability are checked on every page.",
        policy(CredentialCapability::ListingSourcesRead, true, false),
        move |input: AdministeredListingSourcesQuery, context| {
            let service = service.clone();
            async move {
                let request = input.into_request().map_err(invalid)?;
                context
                    .execute(service.execute(&context.operation, request))
                    .await?
                    .map(AdministeredListingSourcesPageData::from)
                    .map_err(error::sources)
            }
        },
    )
}

fn list_operation(
    service: Arc<dyn ListPartnerProductListingsUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listPartnerProductListings",
        "Search authoritative listings within one granted source, including withdrawn listings. Preserves all query, sourceListingId, lifecycle, availability, auctionId, sort/order, size and searchAfter options. Cursors are bound to that source and normalized filters.",
        policy(CredentialCapability::ProductListingsRead, true, false),
        move |input: ListInput, context| {
            let service = service.clone();
            async move {
                let request = input
                    .query
                    .into_partner_request(input.listing_source_id)
                    .map_err(invalid)?;
                context
                    .execute(service.execute(&context.operation, input.listing_source_id, request))
                    .await?
                    .map(SearchResponse::from)
                    .map_err(error::management)
            }
        },
    )
}

fn get_operation(
    service: Arc<dyn GetPartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getPartnerProductListing",
        "Inspect authoritative original listing facts and expectedVersion within a currently granted source, including retained withdrawn listings. Original image URLs are privileged inspection data; previews must respect sensitivity policy.",
        policy(CredentialCapability::ProductListingsRead, true, false),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(
                        &context.operation,
                        input.listing_source_id,
                        input.product_listing_id,
                    ))
                    .await?
                    .map(DetailsData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn update_operation(
    service: Arc<dyn UpdatePartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updatePartnerProductListing",
        "Patch an authoritative listing using its complete expectedVersion. Omitted fields preserve, optional nulls clear, and images: [] clears images. URL, images and the whole auction cannot be null. After a timeout, refetch and reconcile state/version before another command; mutations are never automatically retried.",
        policy(CredentialCapability::ProductListingsWrite, false, true),
        move |input: UpdateInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.product_listing_id)
                    .map_err(invalid)?;
                let command = UpdatePartnerProductListingCommand {
                    listing_source_id: input.listing_source_id,
                    product_listing_id: command.product_listing_id,
                    expected_version: command.expected_version,
                    patch: command.patch,
                };
                context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn withdraw_operation(
    service: Arc<dyn WithdrawPartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "withdrawPartnerProductListing",
        "Withdraw and retain a listing using expectedVersion, returning the resulting identity, version and CHANGED/UNCHANGED outcome. A stale version conflicts. After a timeout, refetch and reconcile state/version before another command.",
        policy(CredentialCapability::ProductListingsWrite, false, true),
        move |input: LifecycleInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(&context.operation, input.into_command()?))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn restore_operation(
    service: Arc<dyn RestorePartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "restorePartnerProductListing",
        "Restore a retained listing to active with unknown availability using expectedVersion. A stale version conflicts; an already-active matching version is unchanged. After a timeout, refetch and reconcile state/version before another command.",
        policy(CredentialCapability::ProductListingsWrite, false, false),
        move |input: LifecycleInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(&context.operation, input.into_command()?))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
            }
        },
    )
}
