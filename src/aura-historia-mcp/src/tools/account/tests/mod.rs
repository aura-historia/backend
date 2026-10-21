use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::operation_context::{CorrelationId, RequestId};
use aura_historia_runtime::Runtime;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;

mod postgres;

fn context(
    owner: UserId,
    capabilities: impl IntoIterator<Item = CredentialCapability>,
) -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: owner,
                capabilities: capabilities.into_iter().collect(),
            },
            request_id: RequestId::new("account-request"),
            correlation_id: CorrelationId::new("account-correlation"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(20),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn registry(runtime: &Runtime) -> Registry {
    Registry::new(
        operations(
            runtime.user_profile().clone(),
            runtime.private_addresses().clone(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn input() -> Value {
    json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Synthetic private home","country":"DE","subdivision":"DE-BE","postalCode":"10115"}})
}
fn requests(id: UserAddressId) -> Vec<(&'static str, Value)> {
    vec![
        ("getUserAccount", json!({})),
        ("updateUserAccount", json!({"firstName":"Ada"})),
        ("createUserAddress", input()),
        ("listUserAddresses", json!({})),
        ("getUserAddress", json!({"addressId":id})),
        (
            "updateUserAddress",
            json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":input()["geography"]}),
        ),
        (
            "deleteUserAddress",
            json!({"addressId":id,"expectedVersion":1}),
        ),
        ("getBrowsingDestination", json!({})),
        (
            "setBrowsingDestination",
            json!({"expectedVersion":1,"destination":{"kind":"NONE"}}),
        ),
    ]
}
async fn invoke(
    registry: &Registry,
    name: &str,
    input: Value,
    context: &InvocationContext,
) -> Value {
    let operation = registry.get(name).unwrap();
    let result = operation
        .call(input, context.clone())
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {error:?}"));
    assert_ne!(Some(true), result.is_error, "{result:?}");
    let output = result.structured_content.unwrap();
    let schema = serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(
        validator.is_valid(&output),
        "{output}: {:?}",
        validator
            .iter_errors(&output)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    output
}

#[tokio::test]
async fn should_publish_exact_family_manifest_schemas_and_truthful_effects() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool);
    let registry = registry(&runtime);
    let manifest: Value =
        serde_json::from_str(include_str!("../../../../authenticated-operations.json")).unwrap();
    let entries = manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["family"] == "account")
        .collect::<Vec<_>>();
    assert_eq!(9, entries.len());
    let (tools, next) = registry.page(None).unwrap();
    assert!(next.is_none());
    assert_eq!(9, tools.len());
    assert_eq!(
        entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>(),
        tools.iter().map(|t| t.name.as_ref()).collect()
    );
    for entry in entries {
        let operation = registry.get(entry["name"].as_str().unwrap()).unwrap();
        let policy = operation.policy();
        assert_eq!(AccessClass::Protected, policy.access);
        assert_eq!(entry["operationId"], entry["name"]);
        assert_eq!(entry["access"], "protected");
        assert_eq!(
            entry["requiredScopes"],
            json!(
                policy
                    .required_scopes
                    .iter()
                    .map(|s| s.as_scope_str())
                    .collect::<Vec<_>>()
            )
        );
        let annotations = operation.tool().annotations.as_ref().unwrap();
        assert_eq!(
            entry["annotations"],
            serde_json::to_value(annotations).unwrap()
        );
        assert!(
            !serde_json::to_string(&operation.tool().input_schema)
                .unwrap()
                .contains("userId")
        );
    }
    let id = UserAddressId::new();
    for (name, input) in requests(id) {
        let schema =
            serde_json::to_value(registry.get(name).unwrap().tool().input_schema.as_ref()).unwrap();
        assert!(
            jsonschema::validator_for(&schema).unwrap().is_valid(&input),
            "{name}: {input}"
        );
    }
    for (name, input) in [
        (
            "updateUserAddress",
            json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":input()["geography"],"owner":UserId::new()}),
        ),
        (
            "deleteUserAddress",
            json!({"addressId":id,"expectedVersion":1,"userId":UserId::new()}),
        ),
        (
            "createUserAddress",
            json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Home","owner":"foreign"}}),
        ),
        (
            "createUserAddress",
            json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}),
        ),
        (
            "createUserAddress",
            json!({"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":null}}),
        ),
        (
            "updateUserAddress",
            json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}),
        ),
        (
            "updateUserAddress",
            json!({"addressId":id,"expectedVersion":1,"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":null}}),
        ),
        (
            "setBrowsingDestination",
            json!({"expectedVersion":1,"destination":{"kind":"NONE","country":"DE"}}),
        ),
        ("getUserAddress", json!({"addressId":id.as_uuid()})),
        ("listUserAddresses", json!({"limit":0})),
    ] {
        let schema =
            serde_json::to_value(registry.get(name).unwrap().tool().input_schema.as_ref()).unwrap();
        assert!(
            !jsonschema::validator_for(&schema).unwrap().is_valid(&input),
            "{name}: {input}"
        );
    }
    for excluded in [
        "deleteUser",
        "deleteUserAccount",
        "createAccessToken",
        "updateAccessToken",
        "createCheckoutSession",
    ] {
        assert!(registry.get(excluded).is_none());
    }
    assert!(Arc::ptr_eq(
        &runtime.user_profile().get,
        &runtime.user_profile().get
    ));
    assert!(Arc::ptr_eq(
        &runtime.user_profile().update,
        &runtime.user_profile().update
    ));
    assert!(Arc::ptr_eq(
        &runtime.private_addresses().commands,
        &runtime.private_addresses().commands
    ));
    assert!(Arc::ptr_eq(
        &runtime.private_addresses().queries,
        &runtime.private_addresses().queries
    ));
    assert_eq!(0, runtime.postgres_pool().size());
}

#[test]
fn should_preserve_technical_error_causes_while_classifying_private_failures() {
    use application::error::static_error;
    use application::transaction::TransactionError;
    use geo_service::resolution::ResolutionError;
    use std::error::Error;
    use user_service::address_book::AddressBookError;
    use user_service::use_cases::{
        commands::update_user_profile::UpdateUserProfileError,
        queries::get_own_user::GetOwnUserError,
    };
    let errors = [
        errors::get_profile(GetOwnUserError::CommitTransactionFailed(static_error(
            "private nested cause",
        ))),
        errors::update_profile(UpdateUserProfileError::BeginTransactionFailed(
            static_error("private nested cause"),
        )),
        errors::address(AddressBookError::Resolution(ResolutionError::Storage(
            static_error("private nested cause"),
        ))),
        errors::address(AddressBookError::Transaction(
            TransactionError::CommitFailed(static_error("private nested cause")),
        )),
        errors::address(AddressBookError::Resolution(ResolutionError::Transaction(
            TransactionError::BeginFailed(static_error("private nested cause")),
        ))),
    ];
    for error in errors {
        assert!(matches!(error, OperationError::Unavailable(_)));
        let mut cause = error.source().unwrap();
        while let Some(source) = cause.source() {
            cause = source;
        }
        assert_eq!("private nested cause", cause.to_string());
        assert!(
            !serde_json::to_string(&error.into_protocol().unwrap())
                .unwrap()
                .contains("private")
        );
    }
    assert!(matches!(
        errors::address(AddressBookError::Conflict),
        OperationError::Conflict
    ));
    assert!(matches!(
        errors::address(AddressBookError::NotFound),
        OperationError::NotFound
    ));
    assert!(matches!(
        errors::address(AddressBookError::InvalidState(static_error(
            "private invalid state"
        ))),
        OperationError::Internal(_)
    ));
    assert!(matches!(
        errors::get_profile(GetOwnUserError::Forbidden),
        OperationError::Forbidden
    ));
}

#[test]
fn should_keep_internal_causes_private_for_each_account_service() {
    use application::error::static_error;
    use geo_service::resolution::ResolutionError;
    use std::error::Error;
    use user_service::{
        address_book::AddressBookError,
        use_cases::{
            commands::update_user_profile::UpdateUserProfileError,
            queries::get_own_user::GetOwnUserError,
        },
    };
    for error in [
        errors::get_profile(GetOwnUserError::InvalidReadModel {
            source: static_error("private nested cause"),
        }),
        errors::update_profile(UpdateUserProfileError::InvalidPersistedState {
            source: static_error("private nested cause"),
        }),
        errors::address(AddressBookError::Resolution(ResolutionError::InvalidState(
            static_error("private nested cause"),
        ))),
    ] {
        assert!(matches!(error, OperationError::Internal(_)));
        let mut cause = error.source().unwrap();
        while let Some(source) = cause.source() {
            cause = source;
        }
        assert_eq!("private nested cause", cause.to_string());
        let protocol = error.into_protocol().unwrap_err();
        assert_eq!(rmcp::model::ErrorCode::INTERNAL_ERROR, protocol.code);
        assert!(
            !serde_json::to_string(&protocol)
                .unwrap()
                .contains("private")
        );
    }
}
