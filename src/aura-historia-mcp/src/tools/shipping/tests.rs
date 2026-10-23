use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aura_historia_runtime::Runtime;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use user_core::user_id::UserId;
mod postgres;

fn context(
    user: UserId,
    scopes: impl IntoIterator<Item = CredentialCapability>,
) -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: user,
                capabilities: scopes.into_iter().collect(),
            },
            request_id: RequestId::new("shipping-test"),
            correlation_id: CorrelationId::new("shipping-test"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn both(user: UserId) -> InvocationContext {
    context(
        user,
        [
            CredentialCapability::ListingSourcesRead,
            CredentialCapability::ListingSourcesWrite,
        ],
    )
}
fn registry(runtime: &Runtime) -> Registry {
    Registry::new(operations(runtime.shipping_management().clone()).unwrap()).unwrap()
}
fn content(amount: u64) -> Value {
    json!({"name":"Shipping MCP terms","attributedPartyId":null,"terms":aura_historia_contracts::shipping::terms::encode(&shipping_core::fixtures::gallery_terms(amount)).unwrap()})
}
fn creation(source: ListingSourceId, key: &str) -> Value {
    json!({"listingSourceId":source,"creationKey":key,"content":content(8000),"evidence":[{"reference":"https://example.test/protected","assertion":"ADVERTISED"}]})
}
async fn invoke(registry: &Registry, name: &str, input: Value, ctx: &InvocationContext) -> Value {
    let op = registry.get(name).unwrap();
    let validator =
        jsonschema::validator_for(&serde_json::to_value(&op.tool().input_schema).unwrap()).unwrap();
    assert!(
        validator.is_valid(&input),
        "{name}: {input}: {:?}",
        validator
            .iter_errors(&input)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let result = op
        .call(input, ctx.clone())
        .await
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    let output = result.structured_content.unwrap();
    let validator =
        jsonschema::validator_for(&serde_json::to_value(&op.tool().output_schema).unwrap())
            .unwrap();
    assert!(
        validator.is_valid(&output),
        "{name}: {output}: {:?}",
        validator
            .iter_errors(&output)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    output
}
async fn assert_error(
    registry: &Registry,
    name: &str,
    input: Value,
    ctx: &InvocationContext,
    expected: fn(&OperationError) -> bool,
) {
    let result = registry.get(name).unwrap().call(input, ctx.clone()).await;
    assert!(result.as_ref().is_err_and(expected), "{name}: {result:?}");
}
#[tokio::test]
async fn should_publish_all_nine_operations_with_matching_schemas_scopes_and_retry_annotations() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool);
    assert!(Arc::ptr_eq(
        &runtime.shipping_management().create,
        &runtime.shipping_management().create
    ));
    let registry = registry(&runtime);
    let manifest: Value =
        serde_json::from_str(include_str!("../../../shipping-operations.json")).unwrap();
    assert_eq!(9, manifest["operations"].as_array().unwrap().len());
    let source = ListingSourceId::new();
    let profile = ShippingProfileId::new();
    let user = UserId::new();
    for entry in manifest["operations"].as_array().unwrap() {
        let name = entry["operationId"].as_str().unwrap();
        let op = registry.get(name).unwrap();
        assert_eq!(entry["name"], op.name());
        assert_eq!(AccessClass::Protected, op.policy().access);
        assert_eq!(
            entry["annotations"],
            serde_json::to_value(op.tool().annotations.as_ref().unwrap()).unwrap()
        );
        assert_eq!(
            entry["requiredScopes"],
            json!(
                op.policy()
                    .required_scopes
                    .iter()
                    .map(|v| v.as_str())
                    .collect::<Vec<_>>()
            )
        );
        let mut input = match name {
            "createShippingProfile" => creation(source, "create"),
            "replaceShippingProfile" => {
                json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1,"content":content(8000),"evidence":[]})
            }
            "retireShippingProfile" | "restoreShippingProfile" => {
                json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1})
            }
            "getShippingProfile" => json!({"listingSourceId":source,"shippingProfileId":profile}),
            "assignShippingDefault" => {
                json!({"listingSourceId":source,"shippingProfileId":profile,"expectedRevision":1})
            }
            "clearShippingDefault" => json!({"listingSourceId":source,"expectedRevision":1}),
            _ => json!({"listingSourceId":source}),
        };
        let validator =
            jsonschema::validator_for(&serde_json::to_value(&op.tool().input_schema).unwrap())
                .unwrap();
        assert!(validator.is_valid(&input), "{name}: {input}");
        // Scope admission is also enforced by the actual use case before touching PostgreSQL.
        assert_error(&registry, name, input.clone(), &context(user, []), |e| {
            matches!(e, OperationError::Forbidden)
        })
        .await;
        input["listingSourceId"] = json!(source.into_uuid().to_string());
        assert!(!validator.is_valid(&input));
        assert_error(&registry, name, input, &both(user), |e| {
            matches!(e, OperationError::InvalidArguments(_))
        })
        .await;
    }
    let mut invalid = creation(source, "missing-attribution");
    invalid["content"]
        .as_object_mut()
        .unwrap()
        .remove("attributedPartyId");
    assert_error(
        &registry,
        "createShippingProfile",
        invalid,
        &both(user),
        |e| matches!(e, OperationError::InvalidArguments(_)),
    )
    .await;
}
#[test]
fn should_preserve_technical_error_causes_and_safe_protocol_classifications() {
    for (unavailable, error) in [
        (
            true,
            shipping_service::ShippingError::PersistenceUnavailable {
                source: application::error::static_error("private persistence failure"),
            },
        ),
        (
            false,
            shipping_service::ShippingError::CorruptShippingState {
                source: application::error::static_error("private corruption"),
            },
        ),
    ] {
        let mapped = errors::shipping(error);
        assert!(std::error::Error::source(&mapped).is_some());
        assert!(!mapped.to_string().contains("private"));
        assert_eq!(
            unavailable,
            matches!(mapped, OperationError::Unavailable(_))
        );
        let protocol = format!("{:?}", mapped.into_protocol());
        assert!(!protocol.contains("private"));
    }
}
