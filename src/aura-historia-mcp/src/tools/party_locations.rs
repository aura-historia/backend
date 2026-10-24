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
mod tests {
    use super::*;
    use application::error::static_error;
    use party_service::use_cases::party_locations::PartyLocationError;
    use serde_json::json;
    use std::error::Error;

    #[test]
    fn should_describe_combined_path_and_body_arguments_without_dropping_business_fields() {
        let party = PartyId::new();
        let id = PartyLocationId::new();
        let v = json!({"partyId":party,"label":"Store","roles":["WAREHOUSE"],"geography":null,"position":null,"evidence":null,"relocates":{"id":id,"revision":2}});
        let schema =
            serde_json::to_value(schemars::schema_for!(PartyArguments<CreateData>)).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(validator.is_valid(&v));
        let input: PartyArguments<CreateData> = serde_json::from_value(v).unwrap();
        assert_eq!(
            id,
            input
                .data
                .into_command(input.party_id)
                .unwrap()
                .relocates
                .unwrap()
                .id
        );
        for value in [
            json!({"partyId":party,"label":"Store","granted":true}),
            json!({"partyId":party,"label":"Store","userId":"actor"}),
            json!({"partyId":party.as_uuid(),"label":"Store"}),
        ] {
            assert!(!validator.is_valid(&value));
            assert!(serde_json::from_value::<PartyArguments<CreateData>>(value).is_err());
        }
        let value = json!({"partyId":party,"locationId":id,"expectedRevision":3,"correction":"SAME_SITE","geography":null});
        let schema =
            serde_json::to_value(schemars::schema_for!(LocationArguments<UpdateData>)).unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        let input: LocationArguments<UpdateData> = serde_json::from_value(value).unwrap();
        let c = input
            .data
            .into_command(input.party_id, input.location_id)
            .unwrap();
        assert_eq!(3, c.expected.revision.into_inner());
    }
    #[test]
    fn should_keep_failure_causes_and_classify_safe_location_errors() {
        assert!(matches!(
            errors::location(PartyLocationError::Forbidden),
            OperationError::Forbidden
        ));
        assert!(matches!(
            errors::location(PartyLocationError::ConcurrencyConflict),
            OperationError::Conflict(None)
        ));
        let error = errors::location(PartyLocationError::TemporarilyUnavailable {
            source: static_error("private cause"),
        });
        assert!(matches!(error, OperationError::Unavailable(_)));
        assert_eq!("private cause", error.source().unwrap().to_string());
        let error = errors::location(PartyLocationError::InvalidPersistedState {
            source: static_error("invalid state"),
        });
        assert!(matches!(error, OperationError::Internal(_)));
        assert_eq!("invalid state", error.source().unwrap().to_string());
    }
}
