//! Protected location management through the existing transaction-owned application use cases.
mod errors;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::party_locations::management::{
    CreateData, LifecycleData, ListData, LocationData, PageData, UpdateData,
};
use aura_historia_runtime::PartyLocations;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use party_service::use_cases::party_locations::{
    CreatePartyLocationUseCase, GetPartyLocationUseCase, ListPartyLocationsUseCase,
    SetPartyLocationLifecycleUseCase, UpdatePartyLocationUseCase,
};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use std::sync::Arc;

fn party_id_schema(_: &mut SchemaGenerator) -> Schema {
    aura_historia_contracts::wire::schema::object_id(PartyId::PREFIX)
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PartyArguments<T> {
    #[schemars(schema_with = "party_id_schema")]
    party_id: PartyId,
    #[serde(flatten)]
    data: T,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocationArguments<T> {
    #[schemars(schema_with = "party_id_schema")]
    party_id: PartyId,
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::party_location_id")]
    location_id: PartyLocationId,
    #[serde(flatten)]
    data: T,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyData {}
fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::PartiesRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn mutation_policy() -> OperationPolicy {
    OperationPolicy {
        required_scopes: vec![CredentialCapability::PartiesWrite],
        read_only: false,
        destructive: true,
        ..read_policy()
    }
}

/// Selects management operations without exposing grant-changing administration.
pub fn operations(services: PartyLocations) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        create(services.create)?,
        list(services.list)?,
        get(services.get)?,
        update(services.update)?,
        lifecycle(services.lifecycle)?,
    ])
}

fn create(
    create: Arc<dyn CreatePartyLocationUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createPartyLocation",
        "Create an operator site with all roles, geography, position and evidence. Requires explicit party-location management authority or eligible partnership membership. Keyless creation assigns a new ID on every accepted call, including identical content. Do not automatically retry an ambiguous timeout; list and reconcile first. relocates atomically retires the old site using its revision and creates a replacement.",
        OperationPolicy {
            idempotent: false,
            ..mutation_policy()
        },
        move |input: PartyArguments<CreateData>, invocation| {
            let create = create.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.party_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(create.execute(&invocation.operation, command))
                    .await?
                    .map(LocationData::from)
                    .map_err(errors::location)
            }
        },
    )
}

fn list(list: Arc<dyn ListPartyLocationsUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listPartyLocations",
        "List protected locations for a party, including revisions, geography and evidence. after continues the bounded keyset page; limit defaults to 25 and must be 1 through 100. Pass returned next as after; authority is rechecked on every page.",
        read_policy(),
        move |input: PartyArguments<ListData>, invocation| {
            let list = list.clone();
            async move {
                let request = input
                    .data
                    .into_request(input.party_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(list.execute(&invocation.operation, request))
                    .await?
                    .map(PageData::from)
                    .map_err(errors::location)
            }
        },
    )
}

fn get(get: Arc<dyn GetPartyLocationUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getPartyLocation",
        "Get one protected party location with its revisions and full evidence. The location must belong to partyId and the caller must hold current location-management authority.",
        read_policy(),
        move |input: LocationArguments<EmptyData>, invocation| {
            let get = get.clone();
            async move {
                invocation
                    .execute(get.execute(
                        &invocation.operation,
                        party_service::use_cases::party_locations::GetPartyLocationRequest {
                            party_id: input.party_id,
                            id: input.location_id,
                        },
                    ))
                    .await?
                    .map(LocationData::from)
                    .map_err(errors::location)
            }
        },
    )
}

fn update(
    update: Arc<dyn UpdatePartyLocationUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "correctPartyLocation",
        "Correct the same site using correction:SAME_SITE and expectedRevision. Omitted fields stay unchanged; null clears geography, position or evidence; label and roles cannot be null. A stale revision conflicts, including on a repeated committed change. Refresh current state after an ambiguous response.",
        mutation_policy(),
        move |input: LocationArguments<UpdateData>, invocation| {
            let update = update.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.party_id, input.location_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(update.execute(&invocation.operation, command))
                    .await?
                    .map(LocationData::from)
                    .map_err(errors::location)
            }
        },
    )
}

fn lifecycle(
    lifecycle: Arc<dyn SetPartyLocationLifecycleUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "setPartyLocationLifecycle",
        "Set ACTIVE or RETIRED using expectedRevision. Retirement excludes current public and inherited geography while retaining historical references. A stale revision conflicts; this operation never selects a replacement default.",
        mutation_policy(),
        move |input: LocationArguments<LifecycleData>, invocation| {
            let lifecycle = lifecycle.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.party_id, input.location_id)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                invocation
                    .execute(lifecycle.execute(&invocation.operation, command))
                    .await?
                    .map(LocationData::from)
                    .map_err(errors::location)
            }
        },
    )
}

#[cfg(test)]
mod tests;
