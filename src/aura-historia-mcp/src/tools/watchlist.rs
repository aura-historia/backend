//! Complete own-watchlist tools; services retain authorization, quotas and presentation policy.
mod error;
#[cfg(test)]
mod tests;

use crate::{
    AccessClass, InvocationContext, OperationDefinition, OperationError, OperationPolicy,
    RegistryError,
};
use application::operation_context::{CredentialCapability, Principal};
use aura_historia_contracts::{
    product_listings::{
        presentation::ProductListingPresentationData,
        product_data::personalized_product_details_data,
    },
    watchlist::{
        CursoredWatchlistData, ListWatchlistQueryData, PatchWatchlistData, PostWatchlistData,
        WatchlistEntryData,
    },
};
use aura_historia_runtime::Watchlist;
use product_listing_core::product_listing_id::ProductListingId;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use user_core::user_id::UserId;
use watchlist_service::use_cases::{
    ListWatchlistUseCase, UnwatchProductListingCommand, UnwatchProductListingUseCase,
    UpdateWatchlistProductListingUseCase, WatchProductListingUseCase,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListInput {
    #[serde(flatten)]
    query: ListWatchlistQueryData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddInput {
    #[serde(flatten)]
    data: PostWatchlistData,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
    #[serde(flatten)]
    data: PatchWatchlistData,
}

pub fn operations(services: Watchlist) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list_operation(services.list)?,
        add_operation(services.add)?,
        remove_operation(services.remove)?,
        update_operation(services.update)?,
    ])
}

fn list_operation(
    service: Arc<dyn ListWatchlistUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getWatchlistProductListings",
        "List your watchlist with complete personalized listing details and notification settings. Supports language, currency and pagination. JSON-encode the returned searchAfter array for the next request.",
        policy(CredentialCapability::WatchlistRead, true, false, true),
        move |input: ListInput, context| {
            let service = service.clone();
            async move {
                let request = input
                    .query
                    .into_request(own_user(&context)?)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.execute(&context.operation, request))
                    .await?
                    .map_err(error::list)?;
                CursoredWatchlistData::try_from(result)
                    .map_err(|error| OperationError::Internal(Box::new(error)))
            }
        },
    )
}

fn add_operation(
    service: Arc<dyn WatchProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "addWatchlistProduct",
        "Add a listing to your watchlist. Notifications default to enabled when omitted or null. Existing entries conflict and tier quotas apply. A timeout can leave the mutation outcome unknown; inspect your watchlist before retrying.",
        policy(CredentialCapability::WatchlistWrite, false, false, true),
        move |input: AddInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(own_user(&context)?)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map(|result| WatchlistEntryData::from(result.entry))
                    .map_err(error::add)
            }
        },
    )
}

fn remove_operation(
    service: Arc<dyn UnwatchProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteWatchlistProduct",
        "Remove a listing from your watchlist. A missing entry returns NOT_FOUND, including after an earlier successful removal. A timeout can leave the mutation outcome unknown; inspect your watchlist before retrying.",
        policy(CredentialCapability::WatchlistWrite, false, true, true),
        move |input: RemoveInput, context| {
            let service = service.clone();
            async move {
                let user_id = own_user(&context)?;
                context
                    .execute(service.execute(
                        &context.operation,
                        UnwatchProductListingCommand {
                            user_id,
                            product_listing_id: input.product_listing_id,
                        },
                    ))
                    .await?
                    .map(|_| ())
                    .map_err(error::remove)
            }
        },
    )
}

fn update_operation(
    service: Arc<dyn UpdateWatchlistProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "patchWatchlistProduct",
        "Update your watchlist notification setting or ACTIVE/INACTIVE_BY_USER state and return the complete personalized listing in the selected language and currency. Omitted fields stay unchanged; null notification/state fields are invalid. Tier quotas apply when activating an entry. A timeout can leave the mutation outcome unknown; inspect your watchlist before retrying.",
        policy(CredentialCapability::WatchlistWrite, false, true, true),
        move |input: UpdateInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(
                        own_user(&context)?,
                        input.product_listing_id,
                        input.presentation,
                    )
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map(|result| personalized_product_details_data(result.product))
                    .map_err(error::update)
            }
        },
    )
}

fn own_user(context: &InvocationContext) -> Result<UserId, OperationError> {
    match &context.operation.principal {
        Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        _ => Err(OperationError::Forbidden),
    }
}

fn policy(
    scope: CredentialCapability,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![scope],
        read_only,
        destructive,
        idempotent,
        open_world: false,
    }
}
