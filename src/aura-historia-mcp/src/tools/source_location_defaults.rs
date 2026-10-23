//! Explicit source-default references and evidence; existing services own eligibility and source grants.
mod errors;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::listing_sources::location_default::{
    AssignData, ChangeData, ClearData, DefaultData,
};
use aura_historia_runtime::SourceLocationDefaults;
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::location_default::{
    AssignLocationDefaultUseCase, ChangeLocationDefaultApplicabilityUseCase,
    ClearLocationDefaultUseCase, GetLocationDefaultUseCase,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceArguments<T> {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: T,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyData {}
fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ListingSourcesRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn mutation_policy() -> OperationPolicy {
    OperationPolicy {
        required_scopes: vec![CredentialCapability::ListingSourcesWrite],
        read_only: false,
        destructive: true,
        ..read_policy()
    }
}
pub fn operations(
    services: SourceLocationDefaults,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        get(services.get)?,
        assign(services.assign)?,
        change(services.change)?,
        clear(services.clear)?,
    ])
}

fn get(get: Arc<dyn GetLocationDefaultUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getSourceLocationDefault",
        "Get the protected source-default reference, revision, eligibility and applicability evidence. Requires current administrator authority or eligible operator-partnership membership with this exact source grant. Returns assignment:null and eligibility:null when unassigned, including after clear.",
        read_policy(),
        move |input: SourceArguments<EmptyData>, invocation| {
            let get = get.clone();
            async move {
                invocation.execute(get.execute(&invocation.operation,listing_source_service::use_cases::location_default::GetLocationDefaultRequest { source_id:input.listing_source_id })).await?.map(DefaultData::from).map_err(errors::default)
            }
        },
    )
}

fn assign(
    assign: Arc<dyn AssignLocationDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "assignSourceLocationDefault",
        "Assign an active partyLocationId owned by the source operator, preserving independent businessAssociation and advertisedInventory evidence. expectedRevision is required, including for no-ops. A stale revision conflicts; inspect current state after an ambiguous response.",
        mutation_policy(),
        move |input: SourceArguments<AssignData>, invocation| {
            let assign = assign.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(assign.execute(&invocation.operation, command))
                    .await?
                    .map(DefaultData::from)
                    .map_err(errors::default)
            }
        },
    )
}

fn change(
    change: Arc<dyn ChangeLocationDefaultApplicabilityUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "changeSourceLocationDefaultApplicability",
        "Change businessAssociation or advertisedInventory evidence at expectedRevision without changing the selected location. Omitted applicability stays unchanged; null clears that meaning. The selected location must be active. Stale revisions conflict.",
        mutation_policy(),
        move |input: SourceArguments<ChangeData>, invocation| {
            let change = change.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(change.execute(&invocation.operation, command))
                    .await?
                    .map(DefaultData::from)
                    .map_err(errors::default)
            }
        },
    )
}

fn clear(
    clear: Arc<dyn ClearLocationDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "clearSourceLocationDefault",
        "Clear the source-default assignment at expectedRevision, retaining its revision tombstone. Clear remains permitted after location retirement. Returns assignment:null and eligibility:null. Stale revisions conflict; an ambiguous response requires inspecting current state.",
        mutation_policy(),
        move |input: SourceArguments<ClearData>, invocation| {
            let clear = clear.clone();
            async move {
                invocation
                    .execute(clear.execute(
                        &invocation.operation,
                        input.data.into_command(input.listing_source_id),
                    ))
                    .await?
                    .map(DefaultData::from)
                    .map_err(errors::default)
            }
        },
    )
}

#[cfg(test)]
mod tests;
