//! Public source reads with optional delegated context, anonymous locations and overview.
mod errors;
mod sources;

use crate::{
    AccessClass, InvocationContext, OperationDefinition, OperationError, OperationPolicy,
    RegistryError,
};
use application::{
    error::static_error,
    operation_context::{OperationContext, Principal},
};
use aura_historia_contracts::{
    party_locations::{DiscoverPartyLocationsData, DiscoverPartyLocationsResultData},
    public_overview::PublicOverviewData,
};
use aura_historia_runtime::{
    PublicListingSourceReadBudget, PublicListingSources, PublicOverview, PublicPartyLocations,
};
use schemars::JsonSchema;
use serde::Deserialize;

/// Family-local allowlist; release composition owns the production registry.
pub fn operations(
    sources: PublicListingSources,
    source_budget: PublicListingSourceReadBudget,
    locations: PublicPartyLocations,
    overview: PublicOverview,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    let mut operations = sources::operations(sources, source_budget)?;
    operations.push(location_operation(locations)?);
    operations.push(overview_operation(overview)?);
    Ok(operations)
}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

fn anonymous_context(invocation: &InvocationContext) -> OperationContext {
    OperationContext {
        principal: Principal::Anonymous,
        ..invocation.operation.clone()
    }
}

fn location_operation(
    services: PublicPartyLocations,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "discoverPartyLocations",
        "Discover public party locations with complete geographic predicates and cursor pagination.",
        read_policy(),
        move |input: DiscoverPartyLocationsData, invocation| {
            let discover = services.discover.clone();
            async move {
                let request = input
                    .try_into()
                    .map_err(|detail| OperationError::InvalidArguments(static_error(detail)))?;
                let result = invocation
                    .execute(discover.execute(request))
                    .await?
                    .map_err(errors::location)?;
                Ok(DiscoverPartyLocationsResultData::from(result))
            }
        },
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OverviewArguments {}

fn overview_operation(services: PublicOverview) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getPublicOverview",
        "Read the stored public catalogue aggregates and their calculation time.",
        read_policy(),
        move |_: OverviewArguments, invocation| {
            let get = services.get.clone();
            async move {
                let context = anonymous_context(&invocation);
                let result = invocation
                    .execute(get.execute(&context))
                    .await?
                    .map_err(errors::overview)?;
                Ok(PublicOverviewData::from(result))
            }
        },
    )
}

#[cfg(test)]
mod tests;
