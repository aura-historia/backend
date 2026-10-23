//! Shipping profile/default operations call the existing authorized use cases in process.
mod errors;
#[cfg(test)]
mod tests;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::shipping::management::{
    self, AssignData, ClearData, CreateData, DefaultData, LifecycleData, ListQuery, ProfileData,
    ReplaceData, SummaryData,
};
use aura_historia_runtime::ShippingManagement;
use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use serde::Deserialize;
use shipping_core::profile::ShippingProfileId;
use shipping_service::use_cases::*;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: CreateData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplaceArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
    #[serde(flatten)]
    data: ReplaceData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "management::profile_id")]
    shipping_profile_id: ShippingProfileId,
    #[serde(flatten)]
    data: LifecycleData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: AssignData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClearArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: ClearData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListArguments {
    #[schemars(schema_with = "management::source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    data: ListQuery,
}

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
fn write_policy(destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ListingSourcesWrite],
        read_only: false,
        destructive,
        idempotent: true,
        open_world: false,
    }
}
/// Shipping management family for explicit composition into the operation registry.
pub fn operations(services: ShippingManagement) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list(services.list)?,
        create(services.create)?,
        get(services.get)?,
        replace(services.replace)?,
        retire(services.retire)?,
        restore(services.restore)?,
        get_default(services.get_default)?,
        assign(services.assign_default)?,
        clear(services.clear_default)?,
    ])
}
fn list(
    service: Arc<dyn ListShippingProfilesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listShippingProfiles",
        "List shipping profiles, including retired profiles. objectKey and optional integrationMethod resolve the caller's import identity. after continues by shippingProfileId; size defaults to 21 and must be 1 through 100.",
        read_policy(),
        move |input: ListArguments, invocation| {
            let service = service.clone();
            async move {
                let after = input.data.after;
                let size = input.data.size.unwrap_or(21);
                let identity = input
                    .data
                    .into_identity()
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(
                        &invocation.operation,
                        input.listing_source_id,
                        after,
                        size,
                        identity,
                    ))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(result
                    .into_iter()
                    .map(SummaryData::from)
                    .collect::<Vec<_>>())
            }
        },
    )
}
fn get(service: Arc<dyn GetShippingProfileUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getShippingProfile",
        "Read complete shipping profile terms, managementRevision, termsRevision and lifecycle for an authorized source.",
        read_policy(),
        move |input: ProfileArguments, invocation| {
            let service = service.clone();
            async move {
                let result = invocation
                    .execute(service.execute(
                        &invocation.operation,
                        input.listing_source_id,
                        input.shipping_profile_id,
                    ))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}
fn get_default(
    service: Arc<dyn GetShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getShippingDefault",
        "Read the source's default shipping profile and its ACTIVE, RETIRED or MISSING eligibility. A cleared default has null profile and eligibility.",
        read_policy(),
        move |input: SourceArguments, invocation| {
            let service = service.clone();
            async move {
                let result = invocation
                    .execute(service.execute(&invocation.operation, input.listing_source_id))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}

fn create(
    service: Arc<dyn CreateShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createShippingProfile",
        "Create a complete shipping profile. creationKey is caller/source scoped: retry identical creation content and evidence with the same key; different content conflicts. Include import provenance for typed imports. attributedPartyId is required and may be null.",
        write_policy(false),
        move |input: CreateArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn replace(
    service: Arc<dyn ReplaceShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "replaceShippingProfile",
        "Replace complete shipping profile content and evidence with expectedRevision. managementRevision and termsRevision are distinct. After an ambiguous result retry identical content, keys and revision; conflicts require refetch and reconciliation.",
        write_policy(true),
        move |input: ReplaceArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn retire(
    service: Arc<dyn RetireShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "retireShippingProfile",
        "Retire a shipping profile using expectedRevision and optional import provenance. Existing defaults retain their reference and report RETIRED eligibility. A stale retry conflicts; refetch and reconcile.",
        write_policy(true),
        move |input: LifecycleArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn restore(
    service: Arc<dyn RestoreShippingProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "restoreShippingProfile",
        "Restore a retired shipping profile using expectedRevision and optional import provenance. A stale retry conflicts; refetch and reconcile.",
        write_policy(false),
        move |input: LifecycleArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id, input.shipping_profile_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                ProfileData::try_from(result).map_err(OperationError::Internal)
            }
        },
    )
}

fn assign(
    service: Arc<dyn AssignShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "assignShippingDefault",
        "Assign an active profile as source default using its shippingProfileId, the default expectedRevision and optional import provenance. Equal assignments still validate current source/profile/location authority.",
        write_policy(true),
        move |input: AssignArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}

fn clear(
    service: Arc<dyn ClearShippingDefaultUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "clearShippingDefault",
        "Clear the source shipping default using expectedRevision and optional import provenance. The retained revision fences stale assignments; a manual clear protects the assertion from later imports.",
        write_policy(true),
        move |input: ClearArguments, invocation| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.listing_source_id)
                    .map_err(OperationError::InvalidArguments)?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::shipping)?;
                Ok(DefaultData::from(result))
            }
        },
    )
}
