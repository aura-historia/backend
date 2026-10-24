//! Source-local delivery inspection, versioned changes and queue admission.
mod error;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    partner_product_listings::admission::{self, AdmissionReportData},
    product_listing_delivery::{self as contract, CommandData, DeliveryData, MutationData},
};
use aura_historia_runtime::{ProductListingDelivery, with_publication_deadline};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingKey, source_listing_id::SourceListingId,
};
use product_listing_service::use_cases::{
    ChangeProductListingDeliveryUseCase, GetProductListingDeliveryUseCase,
    IndexedProductListingIngestionIntent, ProductListingIngestionIntent,
    ProductListingIngestionSubmission, SubmitPartnerProductListingIngestionUseCase,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "contract::source_listing_id_schema")]
    source_listing_id: SourceListingId,
}
impl ListingInput {
    fn into_key(self) -> ProductListingKey {
        ProductListingKey::new(self.listing_source_id, self.source_listing_id)
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangeInput {
    #[serde(flatten)]
    listing: ListingInput,
    #[serde(flatten)]
    command: CommandData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitInput {
    #[serde(flatten)]
    listing: ListingInput,
    #[serde(flatten)]
    command: CommandData,
    #[serde(default)]
    #[schemars(schema_with = "admission::optional_idempotency_key_schema")]
    idempotency_key: Option<String>,
}
fn read_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsRead],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn change_policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsWrite],
        read_only: false,
        destructive: true,
        idempotent: true,
        open_world: false,
    }
}
/// Module-local family; the partner release composes the final catalogue.
pub fn operations(
    services: ProductListingDelivery,
    submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        get(services.get)?,
        change(services.change)?,
        submit_operation(submit)?,
    ])
}
fn get(
    service: Arc<dyn GetProductListingDeliveryUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListingDelivery",
        "Read the complete current protected delivery configuration and expectedVersion. Identify the listing by listingSourceId plus opaque sourceListingId, not the internal productListingId used by management tools. Current source authority and product-listings:read are checked by the use case. A null delivery means source-authored default inheritance.",
        read_policy(),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                let view = context
                    .execute(service.execute(&context.operation, input.into_key()))
                    .await?
                    .map_err(error::delivery)?;
                DeliveryData::try_from(view).map_err(OperationError::Internal)
            }
        },
    )
}
fn change(
    service: Arc<dyn ChangeProductListingDeliveryUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "changeProductListingDelivery",
        "Change delivery only on an existing active listing using listingSourceId, opaque sourceListingId and current positive expectedVersion. Preserve every explicit UNCHANGED/CLEAR/SET patch, integration, selection, rate input, whole-component override, terms reference and optional import provenance. Returns CHANGED or UNCHANGED and the current version. Even semantic no-ops validate current source authority and dependencies. Ordinary stale versions conflict; exact committed import retries retain the existing replay behavior and return current state after authority checks. After an ambiguous response refetch and reconcile, retaining exact import keys/content for an import retry. No automatic retries.",
        change_policy(),
        move |input: ChangeInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .command
                    .into_command(input.listing.into_key())
                    .map_err(OperationError::InvalidArguments)?;
                let result = context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map_err(error::delivery)?;
                Ok(MutationData::from(result))
            }
        },
    )
}
fn submit_operation(
    submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "submitProductListingDelivery",
        "Submit one revision-checked delivery command for FIFO queue admission by listingSourceId and opaque sourceListingId. The report retains originalInputCount 1, original index 0 and the full normalized source-local ID. PUBLISHED confirms queue custody only; source grants, current expectedVersion and business rules are checked when the worker applies it. REJECTED, NOT_ATTEMPTED and UNKNOWN remain distinct; admission never confirms a committed mutation. Optional idempotencyKey is 1-128 visible ASCII bytes excluding comma; omission generates a returned key that cannot be recovered after a lost response. Retry a partial or lost response only with the same actor/source, effective key and unchanged command, including expectedVersion/import fields and original index/count. submissionId is correlation, not a status resource. No automatic retries or exactly-once guarantee.",
        OperationPolicy {
            idempotent: false,
            ..change_policy()
        },
        move |input: SubmitInput, context| {
            let service = submit.clone();
            async move {
                let command = input
                    .command
                    .into_command(input.listing.into_key())
                    .map_err(OperationError::InvalidArguments)?;
                let key = input
                    .idempotency_key
                    .as_deref()
                    .map(admission::parse_idempotency_key)
                    .transpose()
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let source_ids = [Some(command.product_key.source_listing_id.to_string())];
                let submission = ProductListingIngestionSubmission {
                    listing_source_id: command.product_key.listing_source_id,
                    original_input_count: 1,
                    idempotency_key: key,
                    items: vec![IndexedProductListingIngestionIntent {
                        index: 0,
                        intent: ProductListingIngestionIntent::ChangeDelivery(command),
                    }],
                };
                let deadline = context
                    .deadline
                    .checked_sub(Duration::from_millis(500))
                    .unwrap_or_else(tokio::time::Instant::now);
                let result = context
                    .execute(with_publication_deadline(
                        deadline,
                        service.execute(&context.operation, submission),
                    ))
                    .await?
                    .map_err(super::partner_product_listing_ingestion::error::submission)?;
                Ok::<AdmissionReportData, OperationError>(admission::report(
                    result,
                    vec![],
                    &source_ids,
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
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
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
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
                "../../../../docs/fixtures/shipping-imports.json"
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
    async fn should_keep_one_original_item_full_identity_effective_keys_and_stable_retry_commands()
    {
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
    async fn should_match_submission_key_schema_and_runtime_for_omission_null_and_values() {
        let (registry, _, publisher) = fixture(ChangeOutcome::Unchanged);
        let schema =
            serde_json::to_value(&registry.get(SUBMIT).unwrap().tool().input_schema).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let source = ListingSourceId::new();
        for key in [None, Some(Value::Null), Some(json!("explicit-key"))] {
            let mut args = input(source, "opaque");
            if let Some(key) = &key {
                args["idempotencyKey"] = key.clone();
            }
            assert!(validator.is_valid(&args), "{args}");
            let report = call(&registry, SUBMIT, args, context()).await.unwrap();
            match key {
                Some(Value::String(key)) => assert_eq!(report["idempotencyKey"], key),
                _ => assert!(
                    report["idempotencyKey"]
                        .as_str()
                        .unwrap()
                        .starts_with("generated-")
                ),
            }
        }
        for key in [json!(1), json!({}), json!([]), json!(true)] {
            let mut args = input(source, "opaque");
            args["idempotencyKey"] = key;
            assert!(!validator.is_valid(&args));
            assert!(matches!(
                call(&registry, SUBMIT, args, context()).await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert_eq!(publisher.0.lock().unwrap().len(), 3);
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
            "../../../../docs/fixtures/shipping-imports.json"
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
        let ingestion = runtime.partner_product_listing_ingestion(
            ProductListingIngestionDependencies::from_sqs(
                aws_sdk_sqs::Client::from_conf(config),
                "http://127.0.0.1:9/fixture.fifo".into(),
            ),
        );
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

    mod ingress {
        use super::*;

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
        async fn raw_http(registry: Registry, name: &str, args: &str) -> Value {
            use axum::{
                body::{Body, to_bytes},
                http::Request,
            };
            use tower::ServiceExt;
            let app = crate::router(
                crate::TransportConfig {
                    allowed_hosts: vec!["mcp.example.test".into()],
                    ..Default::default()
                },
                registry,
                Arc::new(Gate),
            )
            .unwrap();
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
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap()
        }
        fn full_input(source: ListingSourceId) -> Value {
            let fixture: Value = serde_json::from_str(include_str!(
                "../../../../docs/fixtures/shipping-imports.json"
            ))
            .unwrap();
            let mut body = fixture["providerMembership"].clone();
            let mut terms = fixture["create"]["content"]["terms"].clone();
            terms["collection"]["site"] = json!({"kind":"ASSERTED","geography":null,"position":{"point":{"latitude_degrees":52.5,"longitude_degrees":13.4},"precision":"PREMISES","accuracy_metres":5.0}});
            terms["shipping"]["coverage"]["zones"][0]["services"][0]["rate"]["price"]["amount"]["amount_minor"] =
                json!(u64::MAX);
            body["delivery"]["value"]["selection"] =
                json!({"state":"SET","value":{"kind":"INLINE","terms":terms}});
            body["delivery"]["value"]["shipping"] =
                json!({"state":"SET","value":{"kind":"REPLACE","value":terms["shipping"]}});
            body["delivery"]["value"]["collection"] =
                json!({"state":"SET","value":{"kind":"REPLACE","value":terms["collection"]}});
            body["listingSourceId"] = json!(source);
            body["sourceListingId"] = json!("opaque");
            body
        }
        #[tokio::test]
        async fn should_reject_duplicate_command_fields_through_raw_http_before_any_effect() {
            for name in [CHANGE, SUBMIT] {
                let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
                let body = full_input(ListingSourceId::new()).to_string();
                for (field, duplicate) in [
                    (
                        r#""expectedVersion":1"#,
                        r#""expectedVersion":1,"expectedVersion":2"#,
                    ),
                    (
                        r#""kind":"PROVIDER""#,
                        r#""kind":"PROVIDER","kind":"PROVIDER""#,
                    ),
                    (
                        r#""completeness":"PARTIAL""#,
                        r#""completeness":"PARTIAL","completeness":"COMPLETE""#,
                    ),
                    (
                        r#""latitude_degrees":52.5"#,
                        r#""latitude_degrees":52.5,"latitude_degrees":53.5"#,
                    ),
                    (
                        r#""amount_minor":18446744073709551615"#,
                        r#""amount_minor":1,"amount_minor":18446744073709551615"#,
                    ),
                ] {
                    assert!(body.contains(field), "{field}");
                    let args = body.replacen(field, duplicate, 1);
                    let response = raw_http(registry.clone(), name, &args).await;
                    assert_eq!(
                        response["error"]["code"], -32602,
                        "{name}: {field}: {response}"
                    );
                    assert!(services.writes.lock().unwrap().is_empty());
                    assert!(publisher.0.lock().unwrap().is_empty());
                }
            }
        }
        #[tokio::test]
        async fn should_reject_private_number_objects_in_nested_delivery_values_before_any_effect()
        {
            for name in [CHANGE, SUBMIT] {
                let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
                let body = full_input(ListingSourceId::new()).to_string();
                for (field, forged) in [
                    (
                        r#""latitude_degrees":52.5"#,
                        r#""latitude_degrees":{"$serde_json::private::Number":"52.5"}"#,
                    ),
                    (
                        r#""amount_minor":18446744073709551615"#,
                        r#""amount_minor":{"$serde_json::private::Number":"18446744073709551615"}"#,
                    ),
                ] {
                    let args = body.replacen(field, forged, 1);
                    let response = raw_http(registry.clone(), name, &args).await;
                    assert_eq!(response["error"]["code"], -32602, "{response}");
                    assert!(services.writes.lock().unwrap().is_empty());
                    assert!(publisher.0.lock().unwrap().is_empty());
                }
            }
        }
        #[tokio::test]
        async fn should_preserve_nested_decimal_sites_and_full_width_money_through_raw_http() {
            for name in [CHANGE, SUBMIT] {
                let (registry, services, publisher) = fixture(ChangeOutcome::Changed);
                let source = ListingSourceId::new();
                let mut body = full_input(source);
                body["delivery"]["value"]["rateInputs"] = json!({"state":"UNCHANGED","value":null});
                body["delivery"]["value"]["collection"] =
                    json!({"state":"SET","value":{"kind":"INHERIT","value":null}});
                let response = raw_http(registry, name, &body.to_string()).await;
                assert!(response.get("error").is_none(), "{response}");
                assert_ne!(response["result"]["isError"], true, "{response}");
                body.as_object_mut().unwrap().remove("listingSourceId");
                body.as_object_mut().unwrap().remove("sourceListingId");
                let expected = contract::decode_command(
                    body,
                    ProductListingKey::new(source, "opaque".try_into().unwrap()),
                )
                .unwrap();
                if name == CHANGE {
                    assert_eq!(services.writes.lock().unwrap()[0].1, expected);
                    assert!(publisher.0.lock().unwrap().is_empty());
                } else {
                    assert_eq!(
                        publisher.0.lock().unwrap()[0].intent,
                        ProductListingIngestionIntent::ChangeDelivery(expected)
                    );
                    assert!(services.writes.lock().unwrap().is_empty());
                }
            }
        }
    }

    mod postgres {
        use super::*;
        use aura_historia_contracts::partner_product_listings::CreateProductListingData;
        use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
        use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
        const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
        async fn source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
            let party = uuid::Uuid::now_v7();
            let source = ListingSourceId::new();
            sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Delivery operator')",
    )
    .bind(party)
    .bind(format!("party-{party}"))
    .execute(pool)
    .await
    .unwrap();
            sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Delivery source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
            (party, source)
        }
        async fn grant(
            pool: &sqlx::PgPool,
            user: UserId,
            party: uuid::Uuid,
            source: ListingSourceId,
        ) {
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(user.into_uuid())
                .bind(format!("{user}@example.test"))
                .execute(pool)
                .await
                .unwrap();
            let partnership = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(party)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(user.into_uuid())
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
        }
        async fn create(
            runtime: &Runtime,
            source: ListingSourceId,
            id: &str,
            ctx: &InvocationContext,
        ) -> ProductListingId {
            let command = serde_json::from_value::<CreateProductListingData>(json!({"sourceListingId":id,"title":{"language":"en","text":"Painting"},"url":"https://example.test/painting","images":[]})).unwrap().into_command(source).unwrap();
            runtime
                .partner_product_listing_batches()
                .batches
                .create(&ctx.operation, vec![command])
                .await
                .unwrap()
                .items
                .pop()
                .unwrap()
                .result
                .unwrap()
                .product_listing_id
        }
        fn registry(runtime: &Runtime, publisher: Publisher) -> Registry {
            Registry::new(
                operations(
                    runtime.product_listing_delivery().clone(),
                    runtime
                        .partner_product_listing_ingestion(
                            ProductListingIngestionDependencies::new(publisher),
                        )
                        .submit
                        .clone(),
                )
                .unwrap(),
            )
            .unwrap()
        }
        fn prohibition(source: ListingSourceId, id: &str, version: u64) -> Value {
            json!({"listingSourceId":source,"sourceListingId":id,"expectedVersion":version,"delivery":{"state":"SET","value":{"integration":{"kind":"SOURCE"},"selection":{"state":"UNCHANGED"},"rateInputs":{"state":"UNCHANGED"},"shipping":{"state":"SET","value":{"kind":"REPLACE","value":{"kind":"PROHIBITED"}}},"collection":{"state":"UNCHANGED"}}}})
        }
        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_use_source_local_identity_and_preserve_current_versions_changes_and_stale_noop_conflicts()
         {
            let pool = get_postgres_client().await;
            let (party, source) = source(&pool).await;
            let ctx = context();
            let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
                panic!("delegated")
            };
            grant(&pool, user_id, party, source).await;
            let runtime = Runtime::from_pool(pool.clone());
            let first = create(&runtime, source, "painting", &ctx).await;
            let local_uuid = first.into_uuid().to_string();
            let second = create(&runtime, source, &local_uuid, &ctx).await;
            let registry = registry(&runtime, Publisher::default());
            let query = |id: &str| json!({"listingSourceId":source,"sourceListingId":id});
            let initial = call(&registry, GET, query("painting"), ctx.clone())
                .await
                .unwrap();
            assert_eq!(
                initial,
                json!({"productListingId":first,"expectedVersion":1,"delivery":null})
            );
            let other = call(&registry, GET, query(&local_uuid), ctx.clone())
                .await
                .unwrap();
            assert_eq!(other["productListingId"], json!(second));
            assert_ne!(first, second);
            assert!(matches!(
                call(&registry, GET, query(&first.to_string()), ctx.clone()).await,
                Err(OperationError::NotFound)
            ));
            let changed = call(
                &registry,
                CHANGE,
                prohibition(source, "painting", 1),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(
                changed,
                json!({"productListingId":first,"expectedVersion":2,"outcome":"CHANGED"})
            );
            let unchanged = call(
                &registry,
                CHANGE,
                prohibition(source, "painting", 2),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(
                unchanged,
                json!({"productListingId":first,"expectedVersion":2,"outcome":"UNCHANGED"})
            );
            assert!(matches!(
                call(
                    &registry,
                    CHANGE,
                    prohibition(source, "painting", 1),
                    ctx.clone()
                )
                .await,
                Err(OperationError::Conflict(None))
            ));
            assert!(matches!(
                call(&registry, CHANGE, input(source, "painting"), ctx.clone()).await,
                Err(OperationError::Conflict(None))
            ));
            let current = call(&registry, GET, query("painting"), ctx.clone())
                .await
                .unwrap();
            assert_eq!(current["expectedVersion"], 2);
            assert_eq!(
                current["delivery"]["shipping"],
                json!({"kind":"REPLACE","value":{"kind":"PROHIBITED"}})
            );
            let events:i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1 AND event_group='DOMAIN'").bind(first.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(events, 2);
        }
        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_recheck_grants_for_reads_equal_changes_and_later_application_after_queue_admission()
         {
            let pool = get_postgres_client().await;
            let (party, source) = source(&pool).await;
            let ctx = context();
            let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
                panic!("delegated")
            };
            grant(&pool, user_id, party, source).await;
            let runtime = Runtime::from_pool(pool.clone());
            let id = create(&runtime, source, "painting", &ctx).await;
            let publisher = Publisher::default();
            let registry = registry(&runtime, publisher.clone());
            let (_, foreign) = self::source(&pool).await;
            assert!(matches!(
                call(
                    &registry,
                    GET,
                    json!({"listingSourceId":foreign,"sourceListingId":"painting"}),
                    ctx.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            assert!(matches!(
                call(
                    &registry,
                    CHANGE,
                    prohibition(foreign, "painting", 1),
                    ctx.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            let ordinary = UserId::new();
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(ordinary.into_uuid())
                .bind(format!("{ordinary}@example.test"))
                .execute(&pool)
                .await
                .unwrap();
            let mut consumer = ctx.clone();
            consumer.operation.principal = Principal::DelegatedUser {
                user_id: ordinary,
                capabilities: [
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]
                .into(),
            };
            assert!(matches!(
                call(
                    &registry,
                    GET,
                    json!({"listingSourceId":source,"sourceListingId":"painting"}),
                    consumer.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            assert!(matches!(
                call(&registry, CHANGE, input(source, "painting"), consumer).await,
                Err(OperationError::Forbidden)
            ));
            let report = call(
                &registry,
                SUBMIT,
                prohibition(source, "painting", 1),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(report["items"][0]["outcome"], "PUBLISHED");
            let version: i64 = sqlx::query_scalar(
                "SELECT version FROM product_listings WHERE product_listing_id=$1",
            )
            .bind(id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(version, 1, "queue custody does not mutate a listing");
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(source.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert!(matches!(
                call(
                    &registry,
                    GET,
                    json!({"listingSourceId":source,"sourceListingId":"painting"}),
                    ctx.clone()
                )
                .await,
                Err(OperationError::Forbidden)
            ));
            assert!(matches!(
                call(&registry, CHANGE, input(source, "painting"), ctx.clone()).await,
                Err(OperationError::Forbidden)
            ));
            let ProductListingIngestionIntent::ChangeDelivery(command) =
                publisher.0.lock().unwrap()[0].intent.clone()
            else {
                panic!("delivery intent")
            };
            assert!(matches!(
                runtime
                    .product_listing_delivery()
                    .change
                    .execute(&ctx.operation, command)
                    .await,
                Err(ProductListingDeliveryError::Forbidden)
            ));
        }

        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_recheck_membership_and_source_operator_for_reads_and_equal_changes() {
            let pool = get_postgres_client().await;
            let (party, source) = source(&pool).await;
            let ctx = context();
            let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
                panic!("delegated")
            };
            grant(&pool, user_id, party, source).await;
            let runtime = Runtime::from_pool(pool.clone());
            let id = create(&runtime, source, "painting", &ctx).await;
            let registry = registry(&runtime, Publisher::default());
            call(
                &registry,
                CHANGE,
                prohibition(source, "painting", 1),
                ctx.clone(),
            )
            .await
            .unwrap();
            let partnership: uuid::Uuid = sqlx::query_scalar(
                "DELETE FROM partnership_members WHERE user_id=$1 RETURNING partnership_id",
            )
            .bind(user_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            for operator_changed in [false, true] {
                if operator_changed {
                    sqlx::query(
                        "INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)",
                    )
                    .bind(partnership)
                    .bind(user_id.into_uuid())
                    .execute(&pool)
                    .await
                    .unwrap();
                    let (other, _) = self::source(&pool).await;
                    sqlx::query(
                "UPDATE listing_sources SET operator_party_id=$1 WHERE listing_source_id=$2",
            )
            .bind(other)
            .bind(source.into_uuid())
            .execute(&pool)
            .await
            .unwrap();
                }
                assert!(matches!(
                    call(
                        &registry,
                        GET,
                        json!({"listingSourceId":source,"sourceListingId":"painting"}),
                        ctx.clone()
                    )
                    .await,
                    Err(OperationError::Forbidden)
                ));
                assert!(matches!(
                    call(
                        &registry,
                        CHANGE,
                        prohibition(source, "painting", 2),
                        ctx.clone()
                    )
                    .await,
                    Err(OperationError::Forbidden)
                ));
            }
            let version: i64 = sqlx::query_scalar(
                "SELECT version FROM product_listings WHERE product_listing_id=$1",
            )
            .bind(id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(version, 2);
        }

        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_replay_exact_committed_imports_with_current_version_after_authority_checks()
        {
            let pool = get_postgres_client().await;
            let (party, source) = source(&pool).await;
            let ctx = context();
            let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
                panic!("delegated")
            };
            grant(&pool, user_id, party, source).await;
            let runtime = Runtime::from_pool(pool.clone());
            let id = create(&runtime, source, "painting", &ctx).await;
            let registry = registry(&runtime, Publisher::default());
            let fixture: Value = serde_json::from_str(include_str!(
                "../../../../docs/fixtures/shipping-imports.json"
            ))
            .unwrap();
            let mut args = fixture["membership"].clone();
            args["import"]["acquiredAt"] = json!(
                time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            );
            args["delivery"]["value"]["selection"]["value"]["kind"] = json!("SUPPRESS_INHERITANCE");
            args["listingSourceId"] = json!(source);
            args["sourceListingId"] = json!("painting");
            let result = call(&registry, CHANGE, args.clone(), ctx.clone())
                .await
                .unwrap();
            assert_eq!(
                result,
                json!({"productListingId":id,"expectedVersion":2,"outcome":"CHANGED"})
            );
            let receipts: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM shipping_import_request_fences WHERE listing_source_id=$1",
            )
            .bind(source.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(receipts, 1);
            call(
                &registry,
                CHANGE,
                prohibition(source, "painting", 2),
                ctx.clone(),
            )
            .await
            .unwrap();
            let retry = call(&registry, CHANGE, args.clone(), ctx.clone())
                .await
                .unwrap();
            assert_eq!(
                retry,
                json!({"productListingId":id,"expectedVersion":3,"outcome":"UNCHANGED"})
            );
            let current = call(
                &registry,
                GET,
                json!({"listingSourceId":source,"sourceListingId":"painting"}),
                ctx.clone(),
            )
            .await
            .unwrap();
            assert_eq!(current["expectedVersion"], 3);
            assert_eq!(
                current["delivery"]["shipping"],
                json!({"kind":"REPLACE","value":{"kind":"PROHIBITED"}})
            );
            let events: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1 AND event_group='DOMAIN'")
        .bind(id.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!(events, 3);
            let mut changed_retry = args.clone();
            changed_retry["expectedVersion"] = json!(3);
            assert!(matches!(
                call(&registry, CHANGE, changed_retry, ctx.clone()).await,
                Err(OperationError::Conflict(_))
            ));
            sqlx::query("DELETE FROM partnership_members WHERE user_id=$1")
                .bind(user_id.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            assert!(matches!(
                call(&registry, CHANGE, args, ctx).await,
                Err(OperationError::Forbidden)
            ));
        }
    }
}
