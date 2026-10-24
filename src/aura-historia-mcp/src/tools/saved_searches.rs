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
use aura_historia_contracts::saved_searches::{
    CreateSearchFilterData, PaginatedData, SearchFilterData, SearchFilterMatchData,
    SearchFilterMatchesData, SearchFilterMatchesQuery, UpdateSearchFilterData,
    UpdateSearchFilterMatchFeedbackData, match_cursor_schema,
};
use aura_historia_runtime::SavedSearches;
use product_listing_core::product_listing_id::ProductListingId;
use schemars::JsonSchema;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::{
    CreateSearchFilterUseCase, DeleteOwnedSearchFilterCommand, DeleteOwnedSearchFilterUseCase,
    GetOwnedSearchFilterRequest, GetOwnedSearchFilterUseCase, ListOwnedSearchFiltersRequest,
    ListOwnedSearchFiltersUseCase, ListSearchFilterMatchesUseCase, UpdateOwnedSearchFilterUseCase,
    UpdateSearchFilterMatchFeedbackCommand, UpdateSearchFilterMatchFeedbackUseCase,
};
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
mod tests {
    use super::*;
    use crate::{InvocationContext, Registry};
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, RequestId},
        pagination::{Cursor, CursoredResult},
        patch_field::PatchField,
    };
    use aura_historia_contracts::saved_searches::matches_cursor_value;
    use search_filter_core::search_filter_state::SearchFilterState;
    use search_filter_service::ports::{
        SearchFilterMatchCursor, SearchFilterMatchView, SearchFilterReadError, SearchFilterReader,
        SearchFilterView,
    };
    use search_filter_service::use_cases::{CreateSearchFilterError, UpdateOwnedSearchFilterError};
    use serde_json::json;
    use std::sync::Mutex;
    use time::OffsetDateTime;

    use fixtures::*;

    #[tokio::test]
    async fn should_forward_complete_create_fixture_and_trusted_delegated_context_once() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let input = create_input();
        let schema = Value::Object(
            (*registry
                .get("createUserSearchFilter")
                .unwrap()
                .tool()
                .input_schema)
                .clone(),
        );
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&input));
        let result = registry
            .get("createUserSearchFilter")
            .unwrap()
            .call(
                input.clone(),
                fixture.context(CredentialCapability::SearchFiltersWrite),
            )
            .await
            .unwrap();
        let output = result.structured_content.unwrap();
        assert_eq!(fixture.user.to_string(), output["userId"]);
        assert_eq!(true, output["notifications"]);
        assert_eq!(u64::MAX, output["search"]["price"]["max"]);
        assert_eq!("de", output["search"]["language"]);
        let expected: CreateSearchFilterData = serde_json::from_value(input).unwrap();
        let expected = expected.into_command(fixture.user).unwrap();
        let calls = fixture.calls.lock().unwrap();
        assert_eq!(1, calls.len());
        let Call::Create(context, command) = &calls[0] else {
            panic!("create required")
        };
        assert_eq!(&expected, command);
        assert!(
            matches!(&context.principal,Principal::DelegatedUser {user_id,capabilities} if *user_id==fixture.user && capabilities==&[CredentialCapability::SearchFiltersWrite].into())
        );
        assert_eq!("saved-search-test", context.request_id.to_string());
    }
    #[tokio::test]
    async fn should_reject_null_creation_date_ranges_in_the_published_schema_and_runtime() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let operation = registry.get("createUserSearchFilter").unwrap();
        let schema = Value::Object((*operation.tool().input_schema).clone());
        let schema = jsonschema::validator_for(&schema).unwrap();
        for field in [
            "created",
            "updated",
            "lotBiddingOpens",
            "lotScheduledCloses",
        ] {
            let input = json!({"name":"Saved", "search":{field:null}});
            assert!(!schema.is_valid(&input), "{field}");
            assert!(matches!(
                operation
                    .call(
                        input,
                        fixture.context(CredentialCapability::SearchFiltersWrite)
                    )
                    .await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert!(fixture.calls.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn should_forward_patch_nulls_atomic_availability_and_shipping_acceptance() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let input = json!({"userSearchFilterId":fixture.filter,"name":null,"notifications":false,"state":"INACTIVE_BY_USER","acceptShippingDestination":{"kind":"PREFERENCE"},"search":{"geography":null,"shipping":null,"auctionId":null,"enhancedSearchDescription":null,"availability":[],"orderability":[],"includeUnspecifiedAvailability":false,"language":"de-CH","currency":"JPY"}});
        registry
            .get("updateUserSearchFilter")
            .unwrap()
            .call(
                input,
                fixture.context(CredentialCapability::SearchFiltersWrite),
            )
            .await
            .unwrap();
        {
            let calls = fixture.calls.lock().unwrap();
            let Call::Update(context, command) = &calls[0] else {
                panic!("update required")
            };
            assert_eq!(fixture.user, command.user_id);
            assert_eq!(context.principal.actor_id(), Some(fixture.user.to_string()));
            assert_eq!(PatchField::Unchanged, command.name);
            assert_eq!(PatchField::Set(false), command.notifications);
            assert_eq!(PatchField::Clear, command.search.geography);
            assert_eq!(PatchField::Clear, command.search.shipping);
            assert_eq!(PatchField::Clear, command.search.auction_id_query);
            assert_eq!(
                PatchField::Clear,
                command.search.enhanced_search_description
            );
            assert!(matches!(
                command.search.availability_query,
                PatchField::Set(_)
            ));
            assert!(command.accept_shipping_destination.is_some());
        }
        for search in [
            json!({"language":null}),
            json!({"availability":[]}),
            json!({"availability":null,"orderability":[],"includeUnspecifiedAvailability":false}),
        ] {
            assert!(matches!(
                registry
                    .get("updateUserSearchFilter")
                    .unwrap()
                    .call(
                        json!({"userSearchFilterId":fixture.filter,"search":search}),
                        fixture.context(CredentialCapability::SearchFiltersWrite)
                    )
                    .await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert_eq!(1, fixture.calls.lock().unwrap().len());
    }
    #[tokio::test]
    async fn should_keep_own_reads_complete_and_enforce_independent_scopes() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let result = registry
            .get("getUserSearchFilters")
            .unwrap()
            .call(
                json!({}),
                fixture.context(CredentialCapability::SearchFiltersRead),
            )
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(1, result["total"]);
        assert_eq!(0, result["from"]);
        assert_eq!(1, result["size"]);
        let result = registry
            .get("getUserSearchFilter")
            .unwrap()
            .call(
                json!({"userSearchFilterId":fixture.filter}),
                fixture.context(CredentialCapability::SearchFiltersRead),
            )
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(fixture.filter.to_string(), result["userSearchFilterId"]);
        for name in ["getUserSearchFilters", "getUserSearchFilter"] {
            let input = if name == "getUserSearchFilters" {
                json!({})
            } else {
                json!({"userSearchFilterId":fixture.filter})
            };
            assert!(matches!(
                registry
                    .get(name)
                    .unwrap()
                    .call(
                        input,
                        fixture.context(CredentialCapability::SearchFiltersWrite)
                    )
                    .await,
                Err(OperationError::Forbidden)
            ));
        }
        assert!(matches!(
            registry
                .get("listSearchFilterMatches")
                .unwrap()
                .call(
                    json!({"userSearchFilterId":fixture.filter}),
                    fixture.context(CredentialCapability::SearchFiltersWrite)
                )
                .await,
            Err(OperationError::Forbidden)
        ));
        for (name, input) in [
            ("createUserSearchFilter", create_input()),
            (
                "updateUserSearchFilter",
                json!({"userSearchFilterId":fixture.filter}),
            ),
            (
                "deleteUserSearchFilter",
                json!({"userSearchFilterId":fixture.filter}),
            ),
            (
                "updateSearchFilterMatchFeedback",
                json!({"userSearchFilterId":fixture.filter,"productListingId":"pl_01k6k0w000e008000000000001"}),
            ),
        ] {
            assert!(matches!(
                registry
                    .get(name)
                    .unwrap()
                    .call(
                        input,
                        fixture.context(CredentialCapability::SearchFiltersRead)
                    )
                    .await,
                Err(OperationError::Forbidden)
            ));
        }
    }
    #[tokio::test]
    async fn should_preserve_match_continuation_presentation_and_returned_count() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let cursor = json!([
            "2026-10-21T12:00:00.123456789Z",
            "pl_01k6k0w000e008000000000001"
        ]);
        let output=registry.get("listSearchFilterMatches").unwrap().call(json!({"userSearchFilterId":fixture.filter,"language":"de-CH","currency":"JPY","size":u64::MAX,"searchAfter":cursor}),fixture.context(CredentialCapability::SearchFiltersRead)).await.unwrap().structured_content.unwrap();
        assert_eq!(0, output["size"]);
        assert_eq!(11, output["total"]);
        assert!(output["searchAfter"].is_array());
        let calls = fixture.calls.lock().unwrap();
        let Call::Matches(context, request) = &calls[0] else {
            panic!("matches required")
        };
        assert_eq!(fixture.user, request.user_id);
        assert!(matches!(
            &context.principal,
            Principal::DelegatedUser { .. }
        ));
        assert_eq!(localization::Language::De, request.language);
        assert_eq!(money::Currency::Jpy, request.currency);
        assert_eq!(domain_primitives::sort::SortOrder::Asc, request.order);
        let cursor = request.cursor.as_ref().unwrap();
        assert_eq!(100, cursor.size);
        assert_eq!(
            "2026-10-21T12:00:00.123456789Z",
            matches_cursor_value(cursor.search_after.unwrap()).unwrap()[0]
        );
    }
    #[tokio::test]
    async fn should_keep_feedback_values_snapshots_and_missing_match_semantics() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let listing = "pl_01k6k0w000e008000000000001";
        for (value, expected) in [
            (Some(json!(true)), PatchField::Set(true)),
            (Some(json!(false)), PatchField::Set(false)),
            (Some(Value::Null), PatchField::Clear),
            (None, PatchField::Unchanged),
        ] {
            let mut input = json!({"userSearchFilterId":fixture.filter,"productListingId":listing});
            if let Some(value) = value {
                input["feedback"] = value;
            }
            let output = registry
                .get("updateSearchFilterMatchFeedback")
                .unwrap()
                .call(
                    input,
                    fixture.context(CredentialCapability::SearchFiltersWrite),
                )
                .await
                .unwrap()
                .structured_content
                .unwrap();
            assert_eq!("Captured name", output["userSearchFilterName"]);
            assert!(output.get("shippingMatchValuation").is_some());
            let calls = fixture.calls.lock().unwrap();
            let Call::Feedback(context, command) = calls.last().unwrap() else {
                panic!("feedback required")
            };
            assert_eq!(fixture.user, command.user_id);
            assert!(matches!(
                &context.principal,
                Principal::DelegatedUser { .. }
            ));
            assert_eq!(expected, command.feedback);
        }
        assert!(matches!(registry.get("updateSearchFilterMatchFeedback").unwrap().call(json!({"userSearchFilterId":fixture.filter,"productListingId":ProductListingId::new(),"feedback":true}),fixture.context(CredentialCapability::SearchFiltersWrite)).await,Err(OperationError::NotFound)));
    }
    #[tokio::test]
    async fn should_reject_another_users_saved_search_and_caller_selected_owner() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let other = UserSearchFilterId::new();
        for (name, write, mut input) in [
            (
                "getUserSearchFilter",
                false,
                json!({"userSearchFilterId":other}),
            ),
            (
                "updateUserSearchFilter",
                true,
                json!({"userSearchFilterId":other}),
            ),
            (
                "deleteUserSearchFilter",
                true,
                json!({"userSearchFilterId":other}),
            ),
            (
                "listSearchFilterMatches",
                false,
                json!({"userSearchFilterId":other}),
            ),
            (
                "updateSearchFilterMatchFeedback",
                true,
                json!({"userSearchFilterId":other,"productListingId":"pl_01k6k0w000e008000000000001"}),
            ),
        ] {
            let context = fixture.context(if write {
                CredentialCapability::SearchFiltersWrite
            } else {
                CredentialCapability::SearchFiltersRead
            });
            assert!(matches!(
                registry
                    .get(name)
                    .unwrap()
                    .call(input.clone(), context.clone())
                    .await,
                Err(OperationError::NotFound)
            ));
            input["userId"] = json!(UserId::new());
            assert!(matches!(
                registry.get(name).unwrap().call(input, context).await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        let mut input = create_input();
        input["userId"] = json!(UserId::new());
        assert!(matches!(
            registry
                .get("createUserSearchFilter")
                .unwrap()
                .call(
                    input,
                    fixture.context(CredentialCapability::SearchFiltersWrite)
                )
                .await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    #[tokio::test]
    async fn should_return_null_for_successful_delete_and_preserve_delegated_identity() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let result = registry
            .get("deleteUserSearchFilter")
            .unwrap()
            .call(
                json!({"userSearchFilterId":fixture.filter}),
                fixture.context(CredentialCapability::SearchFiltersWrite),
            )
            .await
            .unwrap()
            .structured_content
            .unwrap();
        assert_eq!(json!({"result":null}), result);
        let calls = fixture.calls.lock().unwrap();
        let Call::Delete(context, command) = &calls[0] else {
            panic!("delete required")
        };
        assert_eq!(fixture.user, command.user_id);
        assert!(matches!(
            &context.principal,
            Principal::DelegatedUser { .. }
        ));
    }
    #[tokio::test]
    async fn should_keep_quota_feature_and_unknown_write_outcomes_distinct_without_retry() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        for (failure, code) in [
            ("quota", "SEARCH_FILTER_QUOTA_EXCEEDED"),
            ("feature", "SEARCH_FILTER_RESTRICTED_FEATURE"),
            ("unavailable", "TEMPORARILY_UNAVAILABLE"),
        ] {
            *fixture.fail_create.lock().unwrap() = Some(failure);
            let before = fixture.calls.lock().unwrap().len();
            let error = registry
                .get("createUserSearchFilter")
                .unwrap()
                .call(
                    create_input(),
                    fixture.context(CredentialCapability::SearchFiltersWrite),
                )
                .await
                .unwrap_err();
            assert_eq!(before + 1, fixture.calls.lock().unwrap().len());
            let result = error.into_protocol().unwrap();
            let value = serde_json::to_value(result).unwrap();
            assert_eq!(code, value["content"][0]["text"]);
            assert!(!value.to_string().contains("private DB"));
        }
    }
    #[test]
    fn should_preserve_technical_causes_and_service_conflicts() {
        use std::error::Error;
        let error = error::create(CreateSearchFilterError::CommitTransactionFailed(
            static_error("private failure"),
        ));
        assert!(error.source().unwrap().source().is_some());
        assert!(matches!(
            error::update(UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict),
            OperationError::Conflict(None)
        ));
        assert!(matches!(
            error::create(CreateSearchFilterError::SearchFilterAlreadyExists),
            OperationError::Conflict(None)
        ));
        assert!(matches!(
            error::update(
                UpdateOwnedSearchFilterError::PersistedSearchFilterStateInvalid {
                    source: static_error("private state")
                }
            ),
            OperationError::Internal(_)
        ));
    }

    #[tokio::test]
    async fn should_preserve_localized_listing_prices_and_scoped_match_state() {
        let fixture = Fixture {
            include_match: true,
            ..Default::default()
        };
        let registry = fixture.registry();
        let operation = registry.get("listSearchFilterMatches").unwrap();
        let output = operation
            .call(
                json!({"userSearchFilterId":fixture.filter,"language":"de-CH","currency":"JPY"}),
                fixture.context(CredentialCapability::SearchFiltersRead),
            )
            .await
            .unwrap()
            .structured_content
            .unwrap();
        let schema = Value::Object((**operation.tool().output_schema.as_ref().unwrap()).clone());
        assert!(
            jsonschema::validator_for(&schema)
                .unwrap()
                .is_valid(&output)
        );
        assert_eq!(1, output["size"]);
        let listing = &output["items"][0];
        assert_eq!("de", listing["item"]["title"]["language"]);
        assert_eq!(
            u64::MAX,
            listing["item"]["pricing"]["display"]["price"]["amount"]
        );
        assert_eq!(
            "JPY",
            listing["item"]["pricing"]["display"]["price"]["currency"]
        );
        assert_eq!(false, listing["userState"]["searchFilter"]["matchFeedback"]);
        assert!(listing["userState"].get("watchlist").is_none());
        assert!(listing["userState"].get("notification").is_none());
        assert!(
            listing["userState"]["searchFilter"]
                .get("shippingMatchValuation")
                .is_some()
        );
    }

    #[tokio::test]
    async fn should_derive_each_concurrent_callers_owner_from_its_own_context() {
        let fixture = Fixture::default();
        let registry = fixture.registry();
        let operation = registry.get("getUserSearchFilter").unwrap();
        let input = json!({"userSearchFilterId":fixture.filter});
        let mut other = fixture.context(CredentialCapability::SearchFiltersRead);
        other.operation.principal = Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: [CredentialCapability::SearchFiltersRead].into(),
        };
        let (own, other) = tokio::join!(
            operation.call(
                input.clone(),
                fixture.context(CredentialCapability::SearchFiltersRead)
            ),
            operation.call(input, other)
        );
        assert!(own.is_ok());
        assert!(matches!(other, Err(OperationError::NotFound)));
    }

    mod fixtures {
        use super::*;
        use search_filter_service::use_cases::{
            CreateSearchFilterCommand, CreateSearchFilterResult, DeleteOwnedSearchFilterError,
            DeleteOwnedSearchFilterResult, GetOwnedSearchFilterHandler,
            ListOwnedSearchFiltersHandler, ListSearchFilterMatchesError,
            ListSearchFilterMatchesRequest, ListSearchFilterMatchesResult,
            UpdateOwnedSearchFilterCommand, UpdateOwnedSearchFilterResult,
            UpdateSearchFilterMatchFeedbackError, UpdateSearchFilterMatchFeedbackResult,
        };
        #[derive(Debug, Clone)]
        pub(super) enum Call {
            Create(OperationContext, CreateSearchFilterCommand),
            Update(OperationContext, UpdateOwnedSearchFilterCommand),
            Delete(OperationContext, DeleteOwnedSearchFilterCommand),
            Matches(OperationContext, ListSearchFilterMatchesRequest),
            Feedback(OperationContext, UpdateSearchFilterMatchFeedbackCommand),
        }
        #[derive(Clone, Default)]
        pub(super) struct Fixture {
            pub(super) user: UserId,
            pub(super) include_match: bool,
            pub(super) filter: UserSearchFilterId,
            pub(super) calls: Arc<Mutex<Vec<Call>>>,
            pub(super) fail_create: Arc<Mutex<Option<&'static str>>>,
        }
        impl Fixture {
            fn view(&self) -> SearchFilterView {
                SearchFilterView {
                    search_filter_id: self.filter,
                    user_id: self.user,
                    name: "Saved".into(),
                    notifications: true,
                    state: SearchFilterState::Active,
                    search: product_listing_core::product_listing_search::ProductListingSearch::new(
                        localization::Language::En,
                        money::Currency::Eur,
                    ),
                    embedding: None,
                    created: OffsetDateTime::UNIX_EPOCH,
                    updated: OffsetDateTime::UNIX_EPOCH,
                }
            }
            fn services(&self) -> SavedSearches {
                SavedSearches {
                    list: Arc::new(ListOwnedSearchFiltersHandler::new(self.clone())),
                    get: Arc::new(GetOwnedSearchFilterHandler::new(self.clone())),
                    create: Arc::new(self.clone()),
                    update: Arc::new(self.clone()),
                    delete: Arc::new(self.clone()),
                    matches: Arc::new(self.clone()),
                    feedback: Arc::new(self.clone()),
                }
            }
            pub(super) fn registry(&self) -> Registry {
                Registry::new(operations(self.services()).unwrap()).unwrap()
            }
            pub(super) fn context(&self, scope: CredentialCapability) -> InvocationContext {
                InvocationContext {
                    operation: OperationContext {
                        principal: Principal::DelegatedUser {
                            user_id: self.user,
                            capabilities: [scope].into(),
                        },
                        request_id: RequestId::new("saved-search-test"),
                        correlation_id: CorrelationId::new("correlation"),
                    },
                    deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
                    cancellation: tokio_util::sync::CancellationToken::new(),
                    max_response_bytes: crate::MAX_RESPONSE_BYTES,
                }
            }
        }
        #[async_trait::async_trait]
        impl SearchFilterReader for Fixture {
            async fn find_for_user(
                &self,
                user_id: UserId,
            ) -> Result<Vec<SearchFilterView>, SearchFilterReadError> {
                Ok(if user_id == self.user {
                    vec![self.view()]
                } else {
                    vec![]
                })
            }
        }
        #[async_trait::async_trait]
        impl CreateSearchFilterUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                command: CreateSearchFilterCommand,
            ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
                context
                    .require()
                    .credential_capability(CredentialCapability::SearchFiltersWrite)
                    .user(&command.user_id)
                    .authorize::<CreateSearchFilterError>()?;
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::Create(context.clone(), command.clone()));
                match *self.fail_create.lock().unwrap() {
                    Some("quota") => {
                        return Err(CreateSearchFilterError::SearchFilterQuotaExceeded {
                            active_count: 2,
                            quota: 2,
                        });
                    }
                    Some("feature") => {
                        return Err(CreateSearchFilterError::SearchFilterFeatureRestricted {
                            feature: "geography",
                        });
                    }
                    Some("unavailable") => {
                        return Err(CreateSearchFilterError::CommitTransactionFailed(
                            static_error("private DB diagnostic"),
                        ));
                    }
                    _ => {}
                }
                let mut filter = self.view();
                filter.name = command.name;
                filter.notifications = command.notifications;
                filter.search = command.search;
                Ok(CreateSearchFilterResult { filter })
            }
        }
        #[async_trait::async_trait]
        impl UpdateOwnedSearchFilterUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                command: UpdateOwnedSearchFilterCommand,
            ) -> Result<UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterError> {
                context
                    .require()
                    .credential_capability(CredentialCapability::SearchFiltersWrite)
                    .user(&command.user_id)
                    .authorize::<UpdateOwnedSearchFilterError>()?;
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::Update(context.clone(), command.clone()));
                if command.search_filter_id != self.filter || command.user_id != self.user {
                    return Err(UpdateOwnedSearchFilterError::SearchFilterNotFound);
                }
                Ok(UpdateOwnedSearchFilterResult {
                    filter: self.view(),
                })
            }
        }
        #[async_trait::async_trait]
        impl DeleteOwnedSearchFilterUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                command: DeleteOwnedSearchFilterCommand,
            ) -> Result<DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterError> {
                context
                    .require()
                    .credential_capability(CredentialCapability::SearchFiltersWrite)
                    .user(&command.user_id)
                    .authorize::<DeleteOwnedSearchFilterError>()?;
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::Delete(context.clone(), command.clone()));
                if command.search_filter_id != self.filter || command.user_id != self.user {
                    return Err(DeleteOwnedSearchFilterError::SearchFilterNotFound);
                }
                Ok(DeleteOwnedSearchFilterResult)
            }
        }
        #[async_trait::async_trait]
        impl ListSearchFilterMatchesUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                request: ListSearchFilterMatchesRequest,
            ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
                context
                    .require()
                    .credential_capability(CredentialCapability::SearchFiltersRead)
                    .user(&request.user_id)
                    .authorize::<ListSearchFilterMatchesError>()?;
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::Matches(context.clone(), request.clone()));
                if request.search_filter_id != self.filter || request.user_id != self.user {
                    return Err(ListSearchFilterMatchesError::SearchFilterNotFound);
                }
                Ok(CursoredResult {
                    items: if self.include_match {
                        vec![matched_listing(
                            self.filter,
                            request.language,
                            request.currency,
                        )]
                    } else {
                        vec![]
                    },
                    cursor: Cursor {
                        size: request.cursor.as_ref().unwrap().size,
                        search_after: Some(SearchFilterMatchCursor {
                            created: OffsetDateTime::UNIX_EPOCH,
                            product_listing_id: Default::default(),
                        }),
                    },
                    total: Some(11),
                })
            }
        }
        #[async_trait::async_trait]
        impl UpdateSearchFilterMatchFeedbackUseCase for Fixture {
            async fn execute(
                &self,
                context: &OperationContext,
                command: UpdateSearchFilterMatchFeedbackCommand,
            ) -> Result<UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackError>
            {
                context
                    .require()
                    .credential_capability(CredentialCapability::SearchFiltersWrite)
                    .user(&command.user_id)
                    .authorize::<UpdateSearchFilterMatchFeedbackError>()?;
                self.calls
                    .lock()
                    .unwrap()
                    .push(Call::Feedback(context.clone(), command.clone()));
                if command.search_filter_id != self.filter || command.user_id != self.user {
                    return Err(UpdateSearchFilterMatchFeedbackError::SearchFilterNotFound);
                }
                if command.product_listing_id.to_string() != "pl_01k6k0w000e008000000000001" {
                    return Err(UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound);
                }
                Ok(UpdateSearchFilterMatchFeedbackResult {
                    search_filter_match: SearchFilterMatchView {
                        user_id: command.user_id,
                        search_filter_id: command.search_filter_id,
                        search_filter_name: Some("Captured name".into()),
                        product_listing_id: command.product_listing_id,
                        origin_event_id: Default::default(),
                        shipping_match_valuation: Some(
                            search_filter_core::ShippingMatchValuation {
                                fx_rate_id: Default::default(),
                                captured_at: OffsetDateTime::UNIX_EPOCH,
                                maximum_per_item: money::Price::new(
                                    100_u64.into(),
                                    money::Currency::Jpy,
                                ),
                            },
                        ),
                        feedback: match command.feedback {
                            PatchField::Set(v) => Some(v),
                            _ => None,
                        },
                        created: OffsetDateTime::UNIX_EPOCH,
                        updated: OffsetDateTime::UNIX_EPOCH,
                    },
                })
            }
        }
        pub(super) fn create_input() -> Value {
            serde_json::from_str(include_str!(
                "../../../aura-historia-contracts/tests/fixtures/saved-search.json"
            ))
            .unwrap()
        }

        fn matched_listing(
            filter_id: UserSearchFilterId,
            language: localization::Language,
            currency: money::Currency,
        ) -> product_listing_service::use_cases::PersonalizedProductListingDetailsView {
            use product_listing_service::use_cases::{
                DisplayProductListingPricing, ProductListingDetailsView, ProductListingImageView,
                ProductListingPricingPresentation, ProductListingPricingValuation,
            };
            let valuation = search_filter_core::ShippingMatchValuation {
                fx_rate_id: Default::default(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
                maximum_per_item: money::Price::new(9200_u64.into(), money::Currency::Eur),
            };
            application::personalized::Personalized {
                item: ProductListingDetailsView {
                    shipping: None,
                    product_listing_id: "pl_01k6k0w000e008000000000001".parse().unwrap(),
                    product_listing_title_slug_id: None,
                    event_id: Default::default(),
                    source: product_listing_service::ports::ListingSourceSummary {
                        listing_source_id: Default::default(),
                        name: "Source".try_into().unwrap(),
                        slug_id: listing_source_core::ListingSourceSlugId::raw("source").unwrap(),
                    },
                    source_listing_id: "merchant:local/42".try_into().unwrap(),
                    product_title: None,
                    product_description: None,
                    title: Some(localization::Localized::new(language, "Schrank".into())),
                    description: None,
                    pricing: ProductListingPricingPresentation {
                        source: product_listing_core::product_listing::ProductListingPricing {
                            price: Some(
                                money::Price::new(u64::MAX.into(), money::Currency::Eur).into(),
                            ),
                            price_estimate_min: None,
                            price_estimate_max: None,
                        },
                        display: DisplayProductListingPricing {
                            price: Some(money::Price::new(u64::MAX.into(), currency).into()),
                            price_estimate_min: None,
                            price_estimate_max: None,
                        },
                        valuation: ProductListingPricingValuation::Current {
                            fx_rate_id: Default::default(),
                            captured_at: OffsetDateTime::UNIX_EPOCH,
                        },
                    },
                    availability: Some(
                        product_listing_core::listing_availability::ListingAvailability::InStock,
                    ),
                    lifecycle: product_listing_core::listing_lifecycle::ListingLifecycle::Active,
                    url: "https://source.example/product/42".parse().unwrap(),
                    view_url: "https://aura.example/product/42".parse().unwrap(),
                    images: vec![ProductListingImageView {
                        url: Some("https://source.example/image.jpg".parse().unwrap()),
                    }],
                    content_policy: None,
                    auction: None,
                    lot: None,
                    created: OffsetDateTime::UNIX_EPOCH,
                    updated: OffsetDateTime::UNIX_EPOCH,
                },
                user_state: Some(
                    product_listing_service::user_state::ProductListingUserStateView {
                        search_filter: Some(
                            product_listing_service::user_state::SearchFilterUserState {
                                matched: true,
                                hidden: false,
                                user_search_filter_id: Some(filter_id),
                                user_search_filter_name: Some("Snapshot name".into()),
                                match_feedback: Some(false),
                                shipping_match_valuation: Some(valuation),
                            },
                        ),
                        ..Default::default()
                    },
                ),
            }
        }
    }
}
