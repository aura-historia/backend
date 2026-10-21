//! Own saved-search CRUD, materialized match reads and feedback.
mod error;
use crate::{
    AccessClass, InvocationContext, OperationDefinition, OperationError, OperationPolicy,
    RegistryError,
};
use application::{
    error::box_error,
    operation_context::{CredentialCapability, Principal},
};
use aura_historia_contracts::saved_searches::*;
use aura_historia_runtime::SavedSearches;
use product_listing_core::product_listing_id::ProductListingId;
use schemars::JsonSchema;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::*;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use user_core::user_id::UserId;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyArguments {}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilterArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArguments {
    #[serde(flatten)]
    data: CreateSearchFilterData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    #[serde(flatten)]
    data: UpdateSearchFilterData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MatchesArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    #[serde(flatten)]
    query: SearchFilterMatchesQuery,
    #[serde(default)]
    #[schemars(schema_with = "match_cursor_schema")]
    search_after: Option<Value>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FeedbackArguments {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    data: UpdateSearchFilterMatchFeedbackData,
}

pub fn operations(services: SavedSearches) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        list(services.list)?,
        create(services.create)?,
        get(services.get)?,
        delete(services.delete)?,
        update(services.update)?,
        matches(services.matches)?,
        feedback(services.feedback)?,
    ])
}
fn owner(invocation: &InvocationContext) -> Result<UserId, OperationError> {
    match &invocation.operation.principal {
        Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        _ => Err(OperationError::Forbidden),
    }
}
fn policy(scope: CredentialCapability, destructive: bool, idempotent: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![scope],
        read_only: scope == CredentialCapability::SearchFiltersRead,
        destructive,
        idempotent,
        open_world: false,
    }
}
fn list(
    service: Arc<dyn ListOwnedSearchFiltersUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserSearchFilters",
        "List every own saved search, including complete criteria, state and notification settings.",
        policy(CredentialCapability::SearchFiltersRead, false, true),
        move |_: EmptyArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let user_id = owner(&invocation)?;
                invocation
                    .execute(service.execute(
                        &invocation.operation,
                        ListOwnedSearchFiltersRequest { user_id },
                    ))
                    .await?
                    .map(|result| PaginatedData::from(result.items))
                    .map_err(error::list)
            }
        },
    )
}
fn create(
    service: Arc<dyn CreateSearchFilterUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "createUserSearchFilter",
        "Create an own saved search with complete criteria and optional shipping-destination acceptance. Notifications default to true. Matching is scheduled and results are eventual. Creation is not idempotent; never automatically retry an unknown outcome.",
        policy(CredentialCapability::SearchFiltersWrite, false, false),
        move |input: CreateArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let command = input
                    .data
                    .into_command(owner(&invocation)?)
                    .map_err(|error| OperationError::InvalidArguments(box_error(error)))?;
                invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map(|result| SearchFilterData::from(result.filter))
                    .map_err(error::create)
            }
        },
    )
}
fn get(
    service: Arc<dyn GetOwnedSearchFilterUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getUserSearchFilter",
        "Get an own saved search with complete criteria, state and notification settings.",
        policy(CredentialCapability::SearchFiltersRead, false, true),
        move |input: FilterArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let request = GetOwnedSearchFilterRequest {
                    user_id: owner(&invocation)?,
                    search_filter_id: input.user_search_filter_id,
                };
                invocation
                    .execute(service.execute(&invocation.operation, request))
                    .await?
                    .map(|result| SearchFilterData::from(result.filter))
                    .map_err(error::get)
            }
        },
    )
}
fn delete(
    service: Arc<dyn DeleteOwnedSearchFilterUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deleteUserSearchFilter",
        "Delete an own saved search. Returns null on success; a missing or other user's search remains not found.",
        policy(CredentialCapability::SearchFiltersWrite, true, true),
        move |input: FilterArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let command = DeleteOwnedSearchFilterCommand {
                    user_id: owner(&invocation)?,
                    search_filter_id: input.user_search_filter_id,
                };
                invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map(|_| ())
                    .map_err(error::delete)
            }
        },
    )
}
fn update(
    service: Arc<dyn UpdateOwnedSearchFilterUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateUserSearchFilter",
        "Patch an own saved search. Nested omission preserves values; supported nulls clear values. Availability, orderability and includeUnspecifiedAvailability must be patched together. Matching remains scheduled and eventual.",
        policy(CredentialCapability::SearchFiltersWrite, true, true),
        move |input: UpdateArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let command = input
                    .data
                    .into_command(owner(&invocation)?, input.user_search_filter_id)
                    .map_err(|error| OperationError::InvalidArguments(box_error(error)))?;
                invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map(|result| SearchFilterData::from(result.filter))
                    .map_err(error::update)
            }
        },
    )
}
fn matches(
    service: Arc<dyn ListSearchFilterMatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listSearchFilterMatches",
        "List materialized matches for an own saved search in ascending match order, with language, currency and JSON searchAfter continuation. Scheduled matching and search projections are eventual; an empty page does not complete background matching.",
        policy(CredentialCapability::SearchFiltersRead, false, true),
        move |input: MatchesArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let request = input
                    .query
                    .into_request(
                        owner(&invocation)?,
                        input.user_search_filter_id,
                        input.search_after,
                    )
                    .map_err(|error| OperationError::InvalidArguments(box_error(error)))?;
                let result = invocation
                    .execute(service.execute(&invocation.operation, request))
                    .await?
                    .map_err(error::matches)?;
                SearchFilterMatchesData::try_from(result)
                    .map_err(|error| OperationError::Internal(box_error(error)))
            }
        },
    )
}
fn feedback(
    service: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updateSearchFilterMatchFeedback",
        "Patch feedback for an existing own saved-search match: true or false sets feedback, null clears it and omission preserves it. Missing or stale matches are not created.",
        policy(CredentialCapability::SearchFiltersWrite, true, true),
        move |input: FeedbackArguments, invocation| {
            let service = Arc::clone(&service);
            async move {
                let command = UpdateSearchFilterMatchFeedbackCommand {
                    user_id: owner(&invocation)?,
                    search_filter_id: input.user_search_filter_id,
                    product_listing_id: input.product_listing_id,
                    feedback: input.data.feedback(),
                };
                invocation
                    .execute(service.execute(&invocation.operation, command))
                    .await?
                    .map(|result| SearchFilterMatchData::from(result.search_filter_match))
                    .map_err(error::feedback)
            }
        },
    )
}
#[cfg(test)]
mod tests;
