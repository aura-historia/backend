//! Current-user profile, private addresses and browsing-destination operations.
mod errors;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use aura_historia_contracts::users::{
    account::{OwnUserData, PatchOwnUserData},
    addresses::{AddressData, CreateData, ExpectedData, PageData, PageQuery, UpdateData},
    destination::{DestinationResponse, SetDestinationData},
};
use aura_historia_runtime::{PrivateAddresses, UserProfile};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use user_core::{user_address::UserAddressId, user_id::UserId};
use user_service::{
    address_book::{AddressBookCommands, AddressBookQueries},
    use_cases::{
        commands::update_user_profile::UpdateUserProfileUseCase,
        queries::get_own_user::{GetOwnUserRequest, GetOwnUserUseCase},
    },
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OwnAccountArguments {}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
    #[serde(flatten)]
    data: UpdateData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeleteAddressArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_address_id")]
    address_id: UserAddressId,
    #[serde(flatten)]
    data: ExpectedData,
}

fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::UsersRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}

fn replacement_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::UsersWrite],
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    }
}

fn creation_policy() -> OperationPolicy {
    OperationPolicy {
        destructive: false,
        idempotent: false,
        ..replacement_policy()
    }
}

fn acting_user(context: &OperationContext) -> Result<UserId, OperationError> {
    match context.principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Ok(id),
        _ => Err(OperationError::Forbidden),
    }
}

/// One family in the shared native/Lambda registry; services own authorization and CAS.
pub fn operations(
    profile: UserProfile,
    addresses: PrivateAddresses,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        get_account(profile.get)?,
        update_account(profile.update)?,
        create_address(addresses.commands.clone())?,
        list_addresses(addresses.queries.clone())?,
        get_address(addresses.queries.clone())?,
        update_address(addresses.commands.clone())?,
        delete_address(addresses.commands.clone())?,
        get_destination(addresses.queries)?,
        set_destination(addresses.commands)?,
    ])
}

fn get_account(get: Arc<dyn GetOwnUserUseCase>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserAccount",
        "Read your complete Aura account and profile preferences.",
        read_policy(),
        move |_: OwnAccountArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.execute(&invocation.operation, GetOwnUserRequest))
                    .await?
                    .map_err(errors::get_profile)?;
                Ok(OwnUserData::from(result))
            }
        },
    )
}

fn update_account(
    update: Arc<dyn UpdateUserProfileUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateUserAccount",
        "Update your profile and preferences. Omitted members are unchanged; null clears optional values. The sensitive-content boolean cannot be null.",
        replacement_policy(),
        move |input: PatchOwnUserData, invocation| {
            let update = update.clone();
            async move {
                let command = input
                    .into_command(acting_user(&invocation.operation)?)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(update.execute(&invocation.operation, command))
                    .await?
                    .map_err(errors::update_profile)?;
                Ok(OwnUserData::from(result.view))
            }
        },
    )
}

fn create_address(
    create: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createUserAddress",
        "Create a private address bookmark with full submitted geography. Every accepted call creates a new identity; after an unknown outcome, reconcile your address book before retrying.",
        creation_policy(),
        move |input: CreateData, invocation| {
            let create = create.clone();
            async move {
                let command = input
                    .into_command()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(create.create(&invocation.operation, command))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn list_addresses(list: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listUserAddresses",
        "List your private addresses, resolution and source attribution. Pass next as after to continue; limit defaults to 25 and accepts 1 through 100.",
        read_policy(),
        move |input: PageQuery, invocation| {
            let list = list.clone();
            async move {
                let result = invocation
                    .execute(list.list(
                        &invocation.operation,
                        input.after,
                        input.limit.unwrap_or(25),
                    ))
                    .await?
                    .map_err(errors::address)?;
                Ok(PageData::from(result))
            }
        },
    )
}

fn get_address(get: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserAddress",
        "Read an owned private address, complete resolution geography and source attribution.",
        read_policy(),
        move |input: AddressArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.get(&invocation.operation, input.address_id))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn update_address(
    update: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateUserAddress",
        "Replace the label and submitted geography of an owned address using its expectedVersion. The address identity and owner are immutable.",
        replacement_policy(),
        move |input: UpdateAddressArguments, invocation| {
            let update = update.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.address_id)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(update.update(&invocation.operation, command))
                    .await?
                    .map_err(errors::address)?;
                Ok(AddressData::from(result))
            }
        },
    )
}

fn delete_address(
    delete: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteUserAddress",
        "Delete the selected owned address using its expectedVersion. If selected for browsing, deletion clears that destination. Returns result:null on success.",
        replacement_policy(),
        move |input: DeleteAddressArguments, invocation| {
            let delete = delete.clone();
            async move {
                invocation
                    .execute(delete.delete(
                        &invocation.operation,
                        input.address_id,
                        input.data.expected_version,
                    ))
                    .await?
                    .map_err(errors::address)
            }
        },
    )
}

fn get_destination(get: Arc<dyn AddressBookQueries>) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getBrowsingDestination",
        "Read your versioned browsing destination: NONE, an owned ADDRESS, or COUNTRY_POSTAL.",
        read_policy(),
        move |_: OwnAccountArguments, invocation| {
            let get = get.clone();
            async move {
                let result = invocation
                    .execute(get.destination(&invocation.operation))
                    .await?
                    .map_err(errors::address)?;
                Ok(DestinationResponse::from(result))
            }
        },
    )
}

fn set_destination(
    set: Arc<dyn AddressBookCommands>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "setBrowsingDestination",
        "Set or clear your browsing destination using its expectedVersion. NONE clears it; ADDRESS must belong to you; COUNTRY_POSTAL accepts a country and optional postalCode.",
        replacement_policy(),
        move |input: SetDestinationData, invocation| {
            let set = set.clone();
            async move {
                let destination = input
                    .destination
                    .into_core()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = invocation
                    .execute(set.set_destination(
                        &invocation.operation,
                        destination,
                        input.expected_version,
                    ))
                    .await?
                    .map_err(errors::address)?;
                Ok(DestinationResponse::from(result))
            }
        },
    )
}

#[cfg(test)]
mod tests;
