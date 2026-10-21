use super::*;
use crate::{InvocationContext, Registry};
use application::{
    error::static_error,
    operation_context::{CorrelationId, OperationContext, RequestId},
    pagination::{Cursor, CursoredResult},
    patch_field::PatchField,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_service::ports::{
    SearchFilterMatchCursor, SearchFilterMatchView, SearchFilterReadError, SearchFilterReader,
    SearchFilterView,
};
use serde_json::json;
use std::sync::Mutex;
use time::OffsetDateTime;

mod fixtures;
use fixtures::*;

#[test]
fn should_register_exact_family_with_shared_schemas_and_effect_metadata() {
    let fixture = Fixture::default();
    let registry = fixture.registry();
    let manifest: Value =
        serde_json::from_str(include_str!("../../../authenticated-operations.json")).unwrap();
    let actual = manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|operation| operation["family"] == "saved_searches")
        .collect::<Vec<_>>();
    assert_eq!(7, actual.len());
    for selected in actual {
        let operation = registry
            .get(selected["operationId"].as_str().unwrap())
            .unwrap();
        let serialized = serde_json::to_value(operation.tool()).unwrap();
        assert_eq!(
            selected["requiredScopes"],
            serialized["_meta"]["aura.historia/requiredScopes"]
        );
        assert_eq!("protected", serialized["_meta"]["aura.historia/access"]);
        assert_eq!(selected["annotations"], serialized["annotations"]);
        jsonschema::validator_for(&Value::Object((*operation.tool().input_schema).clone()))
            .unwrap();
        jsonschema::validator_for(&Value::Object(
            (**operation.tool().output_schema.as_ref().unwrap()).clone(),
        ))
        .unwrap();
    }
}
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
        OperationError::Conflict
    ));
    assert!(matches!(
        error::create(CreateSearchFilterError::SearchFilterAlreadyExists),
        OperationError::Conflict
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
