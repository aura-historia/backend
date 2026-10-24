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
mod tests {
    use super::*;
    use application::error::static_error;
    use listing_source_service::use_cases::location_default::LocationDefaultError;
    use party_core::party_location_id::PartyLocationId;
    use serde_json::json;
    use std::error::Error;
    #[tokio::test]
    async fn should_expose_only_four_default_operations_with_source_scopes_and_revision_annotations()
     {
        let runtime = aura_historia_runtime::Runtime::from_pool(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
                .unwrap(),
        );
        let operations = operations(runtime.source_location_defaults().clone()).unwrap();
        assert_eq!(
            vec![
                "getSourceLocationDefault",
                "assignSourceLocationDefault",
                "changeSourceLocationDefaultApplicability",
                "clearSourceLocationDefault"
            ],
            operations.iter().map(|op| op.name()).collect::<Vec<_>>()
        );
        for op in operations {
            let read = op.name() == "getSourceLocationDefault";
            let annotations = op.tool().annotations.as_ref().unwrap();
            assert_eq!(Some(read), annotations.read_only_hint);
            assert_eq!(Some(!read), annotations.destructive_hint);
            assert_eq!(Some(true), annotations.idempotent_hint);
            assert_eq!(Some(false), annotations.open_world_hint);
            assert_eq!(
                vec![if read {
                    CredentialCapability::ListingSourcesRead
                } else {
                    CredentialCapability::ListingSourcesWrite
                }],
                op.policy().required_scopes
            );
            assert_eq!(AccessClass::Protected, op.policy().access);
        }
    }
    #[test]
    fn should_describe_combined_source_arguments_preserving_null_applicability_and_strict_ids() {
        let source = ListingSourceId::new();
        let id = PartyLocationId::new();
        let value = json!({"listingSourceId":source,"expectedRevision":2,"partyLocationId":id,"businessAssociation":{"reference":"x","scope":"ASSOCIATED_BUSINESS"},"advertisedInventory":null});
        let schema =
            serde_json::to_value(schemars::schema_for!(SourceArguments<AssignData>)).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(validator.is_valid(&value));
        let input: SourceArguments<AssignData> = serde_json::from_value(value).unwrap();
        assert_eq!(
            id,
            input
                .data
                .into_command(input.listing_source_id)
                .unwrap()
                .assignment
                .location_id()
        );
        for value in [
            json!({"listingSourceId":source,"expectedRevision":0,"partyLocationId":id}),
            json!({"listingSourceId":source,"expectedRevision":1,"partyLocationId":id,"granted":true}),
            json!({"listingSourceId":source.as_uuid(),"expectedRevision":1,"partyLocationId":id}),
        ] {
            assert!(!validator.is_valid(&value));
            assert!(serde_json::from_value::<SourceArguments<AssignData>>(value).is_err());
        }
        let value =
            json!({"listingSourceId":source,"expectedRevision":2,"advertisedInventory":null});
        let schema =
            serde_json::to_value(schemars::schema_for!(SourceArguments<ChangeData>)).unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        let input: SourceArguments<ChangeData> = serde_json::from_value(value).unwrap();
        let c = input.data.into_command(input.listing_source_id).unwrap();
        assert_eq!(
            application::patch_field::PatchField::Clear,
            c.advertised_inventory
        );
    }
    #[test]
    fn should_keep_failure_causes_and_classify_safe_default_errors() {
        for error in [
            LocationDefaultError::ConcurrencyConflict,
            LocationDefaultError::LocationRetired,
            LocationDefaultError::DefaultNotAssigned,
        ] {
            assert!(matches!(
                errors::default(error),
                OperationError::Conflict(None)
            ));
        }
        assert!(matches!(
            errors::default(LocationDefaultError::LocationNotFound),
            OperationError::NotFound
        ));
        let error = errors::default(LocationDefaultError::TemporarilyUnavailable {
            source: static_error("private cause"),
        });
        assert!(matches!(error, OperationError::Unavailable(_)));
        assert_eq!("private cause", error.source().unwrap().to_string());
        let error = errors::default(LocationDefaultError::InvalidPersistedState {
            source: static_error("invalid state"),
        });
        assert!(matches!(error, OperationError::Internal(_)));
        assert_eq!("invalid state", error.source().unwrap().to_string());
    }
}
