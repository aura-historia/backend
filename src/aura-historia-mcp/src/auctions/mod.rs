//! Anonymous auction operation family. Release composition owns production registration.
mod error;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::{error::box_error, operation_context::Principal};
use auction_core::AuctionId;
use auction_service::use_cases::{GetPublicAuctionUseCase, ListAuctionsUseCase};
use aura_historia_contracts::auctions::{
    CatalogueData, CatalogueQuery, DirectoryQuery, PublicAuctionData, PublicAuctionDirectoryData,
};
use aura_historia_runtime::PublicAuctions;
use product_listing_service::use_cases::GetAuctionCatalogueUseCase;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::auction_id")]
    auction_id: AuctionId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogueArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::auction_id")]
    auction_id: AuctionId,
    #[serde(flatten)]
    query: CatalogueQuery,
}

/// Builds the complete selected public auction allowlist without enabling it in production.
pub fn operations(services: PublicAuctions) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list(services.list)?,
        get(services.get)?,
        catalogue(services.catalogue)?,
    ])
}

fn list(service: Arc<dyn ListAuctionsUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listAuctions",
        "List public auctions with source, format, reported status, exact schedule range, created or scheduled sorting and continuation filters.",
        policy(),
        move |query: DirectoryQuery, mut invocation| {
            let service = Arc::clone(&service);
            async move {
                let request = query
                    .try_into_request()
                    .map_err(|error| OperationError::InvalidArguments(box_error(error)))?;
                invocation.operation.principal = Principal::Anonymous;
                invocation
                    .execute(service.execute(&invocation.operation, request))
                    .await?
                    .map(PublicAuctionDirectoryData::from)
                    .map_err(error::list)
            }
        },
    )
}

fn get(service: Arc<dyn GetPublicAuctionUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getAuction",
        "Get complete public auction details, source metadata, schedule, counts and catalogue links.",
        policy(),
        move |input: AuctionArguments, mut invocation| {
            let service = Arc::clone(&service);
            async move {
                invocation.operation.principal = Principal::Anonymous;
                let view = invocation
                    .execute(service.execute(&invocation.operation, input.auction_id))
                    .await?
                    .map_err(error::get)?;
                PublicAuctionData::try_from(view)
                    .map_err(|error| OperationError::Internal(box_error(error)))
            }
        },
    )
}

fn catalogue(
    service: Arc<dyn GetAuctionCatalogueUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getAuctionCatalogue",
        "Get public auction lots with language, currency, shipping presentation and stable catalogue continuation.",
        policy(),
        move |input: CatalogueArguments, mut invocation| {
            let service = Arc::clone(&service);
            async move {
                let request = input
                    .query
                    .try_into_request(input.auction_id)
                    .map_err(|error| OperationError::InvalidArguments(box_error(error)))?;
                invocation.operation.principal = Principal::Anonymous;
                invocation
                    .execute(service.execute(&invocation.operation, request))
                    .await?
                    .map(CatalogueData::from)
                    .map_err(error::catalogue)
            }
        },
    )
}

fn policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

#[cfg(test)]
mod tests;
