use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    patch_field::PatchField,
};
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::{delivery::ListingDelivery, product_listing_id::ProductListingId};
use product_listing_service::{
    ports::{
        ProductListingIngestionPublishError, ProductListingIngestionPublisher,
        ProductListingStorageVersion,
    },
    use_cases::*,
};
use serde_json::{Value, json};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use user_core::user_id::UserId;
mod postgres;
const GET: &str = "getProductListingDelivery";
const CHANGE: &str = "changeProductListingDelivery";
const SUBMIT: &str = "submitProductListingDelivery";
struct Delivery {
    outcome: ChangeOutcome,
    reads: Mutex<Vec<(Principal, ProductListingKey)>>,
    writes: Mutex<Vec<(Principal, ChangeProductListingDeliveryCommand)>>,
}
#[async_trait::async_trait]
impl GetProductListingDeliveryUseCase for Delivery {
    async fn execute(
        &self,
        ctx: &OperationContext,
        key: ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError> {
        self.reads
            .lock()
            .unwrap()
            .push((ctx.principal.clone(), key.clone()));
        Ok(ProductListingDeliveryView {
            product_listing_id: ProductListingId::new(),
            version: ProductListingStorageVersion::INITIAL,
            delivery: Some(ListingDelivery::inherited(key.listing_source_id)),
        })
    }
}
#[async_trait::async_trait]
impl ChangeProductListingDeliveryUseCase for Delivery {
    async fn execute(
        &self,
        ctx: &OperationContext,
        command: ChangeProductListingDeliveryCommand,
    ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError> {
        self.writes
            .lock()
            .unwrap()
            .push((ctx.principal.clone(), command));
        Ok(ChangeProductListingDeliveryResult {
            product_listing_id: ProductListingId::new(),
            version: 2u64.try_into().unwrap(),
            outcome: self.outcome,
        })
    }
}
#[derive(Clone, Default)]
struct Publisher(Arc<Mutex<Vec<ProductListingIngestionMessage>>>);
#[async_trait::async_trait]
impl ProductListingIngestionPublisher for Publisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        self.0.lock().unwrap().extend(messages.clone());
        Ok(messages
            .into_iter()
            .filter_map(|message| {
                let id = message.intent.source_listing_id().unwrap().as_ref();
                let outcome = match id {
                    id if id == "unknown" || id.starts_with("unknown-") => {
                        ProductListingIngestionOutcome::Unconfirmed
                    }
                    "missing" => return None,
                    "not-attempted" => ProductListingIngestionOutcome::NotAttempted {
                        reason: ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                        retryable: true,
                    },
                    "rejected" => ProductListingIngestionOutcome::Rejected {
                        reason: ProductListingIngestionRejectionReason::Publisher {
                            code: "INVALID_MESSAGE_SIZE".into(),
                        },
                        retryable: false,
                    },
                    _ => ProductListingIngestionOutcome::Accepted,
                };
                Some(ProductListingIngestionItemOutcome {
                    index: message.metadata.index,
                    command_id: message.metadata.command_id,
                    outcome,
                })
            })
            .collect())
    }
}
fn context() -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: "usr_01h455vb4pex5vy7enb1p677vn".parse().unwrap(),
                capabilities: [
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]
                .into(),
            },
            request_id: RequestId::new("delivery-test"),
            correlation_id: CorrelationId::new("delivery-test"),
        },
        deadline: tokio::time::Instant::now() + Duration::from_secs(10),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn fixture(outcome: ChangeOutcome) -> (Registry, Arc<Delivery>, Publisher) {
    let services = Arc::new(Delivery {
        outcome,
        reads: Mutex::new(vec![]),
        writes: Mutex::new(vec![]),
    });
    let publisher = Publisher::default();
    let registry = Registry::new(
        operations(
            ProductListingDelivery {
                get: services.clone(),
                change: services.clone(),
            },
            Arc::new(SubmitPartnerProductListingIngestionHandler::new(
                publisher.clone(),
            )),
        )
        .unwrap(),
    )
    .unwrap();
    (registry, services, publisher)
}
fn input(source: ListingSourceId, listing: &str) -> Value {
    json!({"listingSourceId":source,"sourceListingId":listing,"expectedVersion":1,"delivery":{"state":"UNCHANGED"}})
}
async fn call(
    registry: &Registry,
    name: &str,
    input: Value,
    context: InvocationContext,
) -> Result<Value, OperationError> {
    let op = registry.get(name).unwrap();
    let value = op
        .call(input.clone(), context)
        .await?
        .structured_content
        .unwrap();
    let input_schema = serde_json::to_value(&op.tool().input_schema).unwrap();
    assert!(
        jsonschema::validator_for(&input_schema)
            .unwrap()
            .is_valid(&input),
        "input schema {name}: {input}"
    );
    let schema = serde_json::to_value(&op.tool().output_schema).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(
        validator.is_valid(&value),
        "{name}: {value}: {:?}",
        validator
            .iter_errors(&value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    Ok(value)
}
#[tokio::test]
async fn should_publish_exact_names_scopes_annotations_and_full_delivery_inputs() {
    for outcome in [ChangeOutcome::Changed, ChangeOutcome::Unchanged] {
        let (registry, services, _) = fixture(outcome);
        assert_eq!(registry.page(None).unwrap().0.len(), 3);
        let source = ListingSourceId::new();
        let mut body: Value = serde_json::from_str(include_str!(
            "../../../../../docs/fixtures/shipping-imports.json"
        ))
        .unwrap();
        body = body["providerMembership"].clone();
        body["listingSourceId"] = json!(source);
        body["sourceListingId"] = json!("  opaque/source #42  ");
        let op = registry.get(CHANGE).unwrap();
        assert_eq!(
            op.policy().required_scopes,
            vec![CredentialCapability::ProductListingsWrite]
        );
        assert!(
            !op.policy().read_only
                && op.policy().destructive
                && op.policy().idempotent
                && !op.policy().open_world
        );
        assert!(
            jsonschema::validator_for(&serde_json::to_value(&op.tool().input_schema).unwrap())
                .unwrap()
                .is_valid(&body)
        );
        let result = call(&registry, CHANGE, body.clone(), context())
            .await
            .unwrap();
        assert_eq!(
            result["outcome"],
            if outcome.changed() {
                "CHANGED"
            } else {
                "UNCHANGED"
            }
        );
        assert_eq!(result["expectedVersion"], 2);
        let writes = services.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, context().operation.principal);
        assert_eq!(
            writes[0].1.product_key,
            ProductListingKey::new(source, "opaque/source #42".try_into().unwrap())
        );
        let mut expected = body;
        expected.as_object_mut().unwrap().remove("listingSourceId");
        expected.as_object_mut().unwrap().remove("sourceListingId");
        assert_eq!(
            writes[0].1,
            contract::decode_command(
                expected,
                ProductListingKey::new(source, "opaque/source #42".try_into().unwrap())
            )
            .unwrap()
        );
        assert_eq!(writes[0].1.import.as_ref().unwrap().sequence, 2);
        let PatchField::Set(patch) = &writes[0].1.delivery else {
            panic!("set patch")
        };
        assert!(matches!(
            patch.selection,
            PatchField::Set(shipping_core::selection::ProfileSelection::Explicit(_))
        ));
        assert!(matches!(patch.rate_inputs, PatchField::Set(_)));
    }
    let (registry, services, _) = fixture(ChangeOutcome::Unchanged);
    let source = ListingSourceId::new();
    for id in [
        ProductListingId::new().to_string(),
        ProductListingId::new().into_uuid().to_string(),
        "é".repeat(256),
    ] {
        let result = call(
            &registry,
            GET,
            json!({"listingSourceId":source,"sourceListingId":id}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!(result["delivery"]["source_id"], json!(source));
        assert_eq!(
            services
                .reads
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .1
                .source_listing_id
                .as_ref(),
            id
        );
    }
    assert_eq!(
        registry.get(GET).unwrap().policy().required_scopes,
        vec![CredentialCapability::ProductListingsRead]
    );
    assert!(registry.get(GET).unwrap().policy().read_only);
    assert!(!registry.get(SUBMIT).unwrap().policy().idempotent);
    for bad in [
        json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":"source-id"}),
        json!({"listingSourceId":source,"productListingId":ProductListingId::new()}),
        json!({"listingSourceId":source,"sourceListingId":"é".repeat(257)}),
    ] {
        assert!(matches!(
            call(&registry, GET, bad, context()).await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    assert_eq!(services.reads.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn should_keep_one_original_item_full_identity_effective_keys_and_stable_retry_commands() {
    let (registry, services, publisher) = fixture(ChangeOutcome::Unchanged);
    let source = ListingSourceId::new();
    for (index, id) in [
        "x".repeat(129),
        "é".repeat(256),
        "  trimmed  ".into(),
        "line\nitem".into(),
    ]
    .into_iter()
    .enumerate()
    {
        let key = format!("delivery-key-{index}");
        let mut args = input(source, &id);
        args["idempotencyKey"] = json!(key);
        let first = call(&registry, SUBMIT, args.clone(), context())
            .await
            .unwrap();
        let retry = call(&registry, SUBMIT, args, context()).await.unwrap();
        assert_eq!(first, retry);
        assert_eq!(first["originalInputCount"], 1);
        assert_eq!(first["acceptedCount"], 1);
        assert_eq!(first["idempotencyKey"], key);
        assert_eq!(
            first["items"],
            json!([{"index":0,"sourceListingId":id.trim(),"outcome":"PUBLISHED","retryable":false}])
        );
        let captured = publisher.0.lock().unwrap();
        let pair = &captured[captured.len() - 2..];
        assert_eq!(pair[0], pair[1]);
        assert_eq!(pair[0].metadata.index, 0);
        assert_eq!(pair[0].metadata.input_count, 1);
        assert_eq!(
            pair[0].metadata.operation,
            ProductListingIngestionOperation::ChangeDelivery
        );
        let ProductListingIngestionIntent::ChangeDelivery(command) = &pair[0].intent else {
            panic!("delivery intent")
        };
        assert_eq!(command.product_key.source_listing_id.as_ref(), id.trim());
        assert_eq!(
            command.expected_version,
            ProductListingStorageVersion::INITIAL
        );
        assert_eq!(command.delivery, PatchField::Unchanged);
    }
    let generated = call(&registry, SUBMIT, input(source, "generated"), context())
        .await
        .unwrap();
    assert!(
        generated["idempotencyKey"]
            .as_str()
            .unwrap()
            .starts_with("generated-")
    );
    assert!(services.writes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn should_keep_unknown_unsent_and_rejected_admission_distinct_from_committed_mutation() {
    let (registry, services, publisher) = fixture(ChangeOutcome::Unchanged);
    for (id, outcome, error, retryable) in [
        ("unknown", "UNKNOWN", "ENQUEUE_UNCONFIRMED", true),
        ("missing", "UNKNOWN", "ENQUEUE_UNCONFIRMED", true),
        (
            "not-attempted",
            "NOT_ATTEMPTED",
            "ENQUEUE_NOT_ATTEMPTED",
            true,
        ),
        (
            "rejected",
            "REJECTED",
            "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
            false,
        ),
    ] {
        let report = call(
            &registry,
            SUBMIT,
            input(ListingSourceId::new(), id),
            context(),
        )
        .await
        .unwrap();
        assert_eq!(report["acceptedCount"], 0);
        assert_eq!(report["items"][0]["outcome"], outcome);
        assert_eq!(report["items"][0]["error"], error);
        assert_eq!(report["items"][0]["retryable"], retryable);
        assert!(report.get("productListingId").is_none());
    }
    let long_id = format!("unknown-{}", "x".repeat(129));
    let report = call(
        &registry,
        SUBMIT,
        input(ListingSourceId::new(), &long_id),
        context(),
    )
    .await
    .unwrap();
    assert_eq!(report["items"][0]["sourceListingId"], long_id);
    assert_eq!(report["items"][0]["outcome"], "UNKNOWN");
    assert!(report["failures"][0].get("sourceListingId").is_none());
    for key in ["", "with spaces", "folded,key", &"x".repeat(129)] {
        let mut args = input(ListingSourceId::new(), "invalid");
        args["idempotencyKey"] = json!(key);
        assert!(matches!(
            call(&registry, SUBMIT, args, context()).await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    assert_eq!(publisher.0.lock().unwrap().len(), 5);
    assert!(services.writes.lock().unwrap().is_empty());
}
#[tokio::test]
async fn should_enforce_missing_credential_scopes_through_the_existing_use_cases_before_io() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    let runtime = aura_historia_runtime::Runtime::from_pool(pool);
    assert!(Arc::ptr_eq(
        &runtime.product_listing_delivery().get,
        &runtime.product_listing_delivery().get
    ));
    assert!(Arc::ptr_eq(
        &runtime.product_listing_delivery().change,
        &runtime.product_listing_delivery().change
    ));
    let registry = Registry::new(
        operations(
            runtime.product_listing_delivery().clone(),
            Arc::new(SubmitPartnerProductListingIngestionHandler::new(
                Publisher::default(),
            )),
        )
        .unwrap(),
    )
    .unwrap();
    for principal in [
        Principal::Anonymous,
        Principal::DelegatedUser {
            user_id: UserId::new(),
            capabilities: Default::default(),
        },
    ] {
        for name in [GET, CHANGE, SUBMIT] {
            let mut ctx = context();
            ctx.operation.principal = principal.clone();
            let mut args = input(ListingSourceId::new(), "opaque");
            if name == GET {
                args.as_object_mut().unwrap().remove("expectedVersion");
                args.as_object_mut().unwrap().remove("delivery");
            }
            assert!(matches!(
                call(&registry, name, args, ctx).await,
                Err(OperationError::Forbidden)
            ));
        }
    }
}
#[derive(Clone)]
struct Gate;
#[async_trait::async_trait]
impl crate::AdmissionGate for Gate {
    async fn admit(
        &self,
        _: crate::AdmissionRequest<'_>,
    ) -> Result<Principal, crate::AdmissionRejection> {
        Ok(context().operation.principal)
    }
}
#[tokio::test]
async fn should_reject_duplicate_command_fields_through_raw_http_before_any_effect() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    for name in [CHANGE, SUBMIT] {
        let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
        let app = crate::router(
            crate::TransportConfig {
                allowed_hosts: vec!["mcp.example.test".into()],
                ..Default::default()
            },
            registry,
            Arc::new(Gate),
        )
        .unwrap();
        let args = input(ListingSourceId::new(), "opaque")
            .to_string()
            .replacen("{", r#"{"expectedVersion":2,"#, 1);
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1")
                    .header("host", "mcp.example.test")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(request))
                    .unwrap(),
            )
            .await
            .unwrap();
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(services.writes.lock().unwrap().is_empty());
        assert!(publisher.0.lock().unwrap().is_empty());
    }
}
#[test]
fn should_preserve_delivery_technical_causes_and_safe_error_classes() {
    use std::error::Error;
    for (unavailable, error) in [
        (
            true,
            ProductListingDeliveryError::Transaction(application::error::static_error(
                "private transaction cause",
            )),
        ),
        (
            false,
            ProductListingDeliveryError::OperationFailed {
                source: application::error::static_error("private persisted payload"),
            },
        ),
    ] {
        let mapped = error::delivery(error);
        assert!(mapped.source().is_some());
        assert_eq!(
            matches!(mapped, OperationError::Unavailable(_)),
            unavailable
        );
        assert!(!format!("{:?}", mapped.into_protocol()).contains("private"));
    }
    assert!(matches!(
        error::delivery(ProductListingDeliveryError::StaleRevision),
        OperationError::Conflict(None)
    ));
    assert!(matches!(
        error::delivery(ProductListingDeliveryError::ReferenceIneligible),
        OperationError::Conflict(None)
    ));
}

#[tokio::test]
async fn should_submit_the_complete_shared_delivery_command_without_dropping_import_or_rate_fields()
{
    let (registry, _, publisher) = fixture(ChangeOutcome::Unchanged);
    let source = ListingSourceId::new();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../docs/fixtures/shipping-imports.json"
    ))
    .unwrap();
    let mut body = fixture["providerMembership"].clone();
    let mut terms = fixture["create"]["content"]["terms"].clone();
    terms["collection"]["site"] = json!({"kind":"ASSERTED","geography":null,"position":{"point":{"latitude_degrees":52.5,"longitude_degrees":13.4},"precision":"PREMISES","accuracy_metres":5.0}});
    body["delivery"]["value"]["selection"] =
        json!({"state":"SET","value":{"kind":"INLINE","terms":terms}});
    let expected = contract::decode_command(
        body.clone(),
        ProductListingKey::new(source, "provider-item".try_into().unwrap()),
    )
    .unwrap();
    body["listingSourceId"] = json!(source);
    body["sourceListingId"] = json!("provider-item");
    let report = call(&registry, SUBMIT, body, context()).await.unwrap();
    assert_eq!(report["items"][0]["outcome"], "PUBLISHED");
    let messages = publisher.0.lock().unwrap();
    assert_eq!(
        messages[0].intent,
        ProductListingIngestionIntent::ChangeDelivery(expected)
    );
}
#[tokio::test]
async fn should_reserve_response_time_for_each_publication_using_the_warm_runtime_publisher() {
    use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool);
    let config = aws_sdk_sqs::config::Builder::new()
        .behavior_version_latest()
        .region(aws_sdk_sqs::config::Region::new("eu-central-1"))
        .credentials_provider(aws_sdk_sqs::config::Credentials::new(
            "fixture", "fixture", None, None, "fixture",
        ))
        .build();
    let ingestion =
        runtime.partner_product_listing_ingestion(ProductListingIngestionDependencies::from_sqs(
            aws_sdk_sqs::Client::from_conf(config),
            "http://127.0.0.1:9/fixture.fifo".into(),
        ));
    let registry = Registry::new(
        operations(
            runtime.product_listing_delivery().clone(),
            ingestion.submit.clone(),
        )
        .unwrap(),
    )
    .unwrap();
    for key in ["first", "second"] {
        let mut ctx = context();
        ctx.deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        let deadline = ctx.deadline;
        let mut args = input(ListingSourceId::new(), "unsent");
        args["idempotencyKey"] = json!(key);
        let result = call(&registry, SUBMIT, args, ctx).await.unwrap();
        assert_eq!(result["acceptedCount"], 0);
        assert_eq!(result["items"][0]["outcome"], "NOT_ATTEMPTED");
        assert_eq!(result["items"][0]["error"], "ENQUEUE_NOT_ATTEMPTED");
        assert!(tokio::time::Instant::now() < deadline);
    }
}
