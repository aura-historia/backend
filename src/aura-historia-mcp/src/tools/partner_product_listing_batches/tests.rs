use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::patch_field::PatchField;
use domain_primitives::change_outcome::ChangeOutcome;
use product_listing_core::{
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::use_cases::{
    CreateProductListingCommand, CreateProductListingError, CreateProductListingResult,
    CreateProductListingUseCase, UpdateProductListingCommand, UpdateProductListingError,
    UpdateProductListingResult, UpdateProductListingUseCase, UpsertProductListingCommand,
    UpsertProductListingError, UpsertProductListingResult, UpsertProductListingUseCase,
    WithdrawProductListingError, WithdrawProductListingResult, WithdrawProductListingUseCase,
    commands::partner_product_listing_batches::PartnerProductListingBatchesHandler,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod postgres;
const NAMES: [&str; 4] = [
    "postPartnerProductListings",
    "patchPartnerProductListings",
    "putPartnerProductListings",
    "deletePartnerProductListings",
];
const PRIVATE_COMMIT_CAUSE: &str = "private transaction failure detail";
#[derive(Debug)]
enum Call {
    Create(Box<CreateProductListingCommand>),
    Update(ProductListingKey, Box<UpdateProductListingCommand>),
    Upsert(Box<UpsertProductListingCommand>),
    Withdraw(ProductListingKey),
}
#[derive(Default)]
struct Items {
    calls: Mutex<Vec<Call>>,
}
fn created() -> CreateProductListingResult {
    CreateProductListingResult {
        product_listing_id: ProductListingId::new(),
        product_listing_title_slug_id: ProductListingSlugId::raw("created-a1b2c3").unwrap(),
    }
}
#[async_trait::async_trait]
impl CreateProductListingUseCase for Items {
    async fn execute(
        &self,
        _: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        let source_listing_id = command.source_listing_id.to_string();
        self.calls
            .lock()
            .unwrap()
            .push(Call::Create(Box::new(command)));
        match source_listing_id.as_str() {
            "uncertain" => Err(CreateProductListingError::CommitTransactionFailed(
                application::error::static_error(PRIVATE_COMMIT_CAUSE),
            )),
            "fail" => Err(CreateProductListingError::SourceListingAlreadyExists),
            _ => Ok(created()),
        }
    }
}
#[async_trait::async_trait]
impl UpdateProductListingUseCase for Items {
    async fn execute(
        &self,
        _: &OperationContext,
        _: ProductListingId,
        _: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        unreachable!()
    }
    async fn execute_by_key(
        &self,
        _: &OperationContext,
        key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        let source_listing_id = key.source_listing_id.to_string();
        self.calls
            .lock()
            .unwrap()
            .push(Call::Update(key, Box::new(command)));
        match source_listing_id.as_str() {
            "uncertain" => Err(UpdateProductListingError::CommitTransactionFailed(
                application::error::static_error(PRIVATE_COMMIT_CAUSE),
            )),
            "fail" => Err(UpdateProductListingError::NotFound),
            _ => Ok(UpdateProductListingResult {
                product_listing_id: ProductListingId::new(),
                outcome: ChangeOutcome::Changed,
            }),
        }
    }
}
#[async_trait::async_trait]
impl UpsertProductListingUseCase for Items {
    async fn execute(
        &self,
        _: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        let source_listing_id = command.source_listing_id.to_string();
        self.calls
            .lock()
            .unwrap()
            .push(Call::Upsert(Box::new(command)));
        match source_listing_id.as_str() {
            "uncertain" => Err(UpsertProductListingError::CommitTransactionFailed(
                application::error::static_error(PRIVATE_COMMIT_CAUSE),
            )),
            "fail" => Err(UpsertProductListingError::InvalidProductListing {
                source: application::error::static_error("private provider content"),
            }),
            _ => Ok(UpsertProductListingResult::Created(created())),
        }
    }
}
#[async_trait::async_trait]
impl WithdrawProductListingUseCase for Items {
    async fn execute(
        &self,
        _: &OperationContext,
        _: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        unreachable!()
    }
    async fn execute_by_key(
        &self,
        _: &OperationContext,
        key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        let source_listing_id = key.source_listing_id.to_string();
        self.calls.lock().unwrap().push(Call::Withdraw(key));
        match source_listing_id.as_str() {
            "uncertain" => Err(WithdrawProductListingError::CommitTransactionFailed(
                application::error::static_error(PRIVATE_COMMIT_CAUSE),
            )),
            "fail" => Err(WithdrawProductListingError::NotFound),
            _ => Ok(WithdrawProductListingResult {
                product_listing_id: ProductListingId::new(),
                outcome: ChangeOutcome::Unchanged,
            }),
        }
    }
}
fn context() -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: user_core::user_id::UserId::new(),
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            },
            request_id: RequestId::new("batches"),
            correlation_id: CorrelationId::new("batches"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(30),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}
fn fixture() -> (Registry, Arc<Items>) {
    let items = Arc::new(Items::default());
    let batches = Arc::new(PartnerProductListingBatchesHandler::new(
        items.clone(),
        items.clone(),
        items.clone(),
        items.clone(),
    ));
    (
        Registry::new(operations(PartnerProductListingBatches { batches }).unwrap()).unwrap(),
        items,
    )
}
fn product(id: &str) -> Value {
    json!({"sourceListingId":id,"title":{"text":"Cabinet","language":"en"},"description":{"text":"Schrank","language":"de"},"price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX},"priceEstimateMin":{"currency":"USD","amount":0},"priceEstimateMax":{"currency":"GBP","amount":u64::MAX},"availability":"AVAILABLE","url":"https://source.test/listing","images":["https://source.test/one.jpg","https://source.test/two.jpg"],"auction":{"auctionId":null,"lotNumber":"Lot 42","cataloguePosition":u32::MAX,"timing":{"biddingOpens":null,"scheduledCloses":"2026-10-23T12:00:00Z","reportedClosedAt":null}}})
}
fn input_product(name: &str, id: &str) -> Value {
    let mut item = product(id);
    if name == NAMES[1] {
        item.as_object_mut().unwrap().remove("title");
        item.as_object_mut().unwrap().remove("description");
    }
    if name == NAMES[3] {
        item = json!({"sourceListingId":id,"ignoredLegacyMember":true});
    }
    item
}
async fn call(
    registry: &Registry,
    name: &str,
    input: Value,
    context: InvocationContext,
) -> Result<Value, OperationError> {
    let operation = registry.get(name).unwrap();
    let schema = serde_json::to_value(&*operation.tool().input_schema).unwrap();
    assert!(
        jsonschema::validator_for(&schema).unwrap().is_valid(&input),
        "input schema: {input}"
    );
    let result = operation.call(input, context).await?;
    let value = result.structured_content.unwrap();
    let schema = serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
    assert!(
        jsonschema::validator_for(&schema).unwrap().is_valid(&value),
        "output schema: {value}"
    );
    Ok(value)
}
#[tokio::test]
async fn should_expose_four_stable_names_and_preserve_every_supported_field() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let operation = registry.get(name).unwrap();
        assert_eq!(AccessClass::Protected, operation.policy().access);
        assert!(!operation.policy().read_only);
        assert!(!operation.policy().idempotent);
        assert_eq!(
            vec![CredentialCapability::ProductListingsWrite],
            operation.policy().required_scopes
        );
        let value = call(
            &registry,
            name,
            json!({"listingSourceId":source,"products":[input_product(name,"  full  ")]}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!("COMPLETED", value["status"]);
        assert_eq!(1, value["succeededCount"]);
        assert_eq!(0, value["failedCount"]);
        assert_eq!("  full  ", value["successes"][0]["sourceListingId"]);
    }
    let calls = items.calls.lock().unwrap();
    let Call::Create(command) = &calls[0] else {
        panic!("create command")
    };
    assert_eq!(source, command.listing_source_id);
    assert_eq!("full", command.source_listing_id.as_ref());
    assert_eq!("Cabinet", command.title.as_ref().unwrap().payload.as_ref());
    assert_eq!(
        localization::Language::De,
        command.description.as_ref().unwrap().localization
    );
    assert_eq!(2, command.images.len());
    assert_eq!(
        u64::MAX,
        u64::from(command.pricing.price_estimate_max.unwrap().monetary_amount)
    );
    let auction = command.auction.as_ref().unwrap();
    assert_eq!(PatchField::Clear, auction.auction_id);
    assert_eq!(PatchField::Clear, auction.bidding_opens);
    assert!(matches!(auction.catalogue_position, PatchField::Set(_)));
    assert!(matches!(auction.scheduled_closes, PatchField::Set(_)));
    let Call::Update(key, command) = &calls[1] else {
        panic!("update command")
    };
    assert_eq!(source, key.listing_source_id);
    assert!(matches!(command.price, PatchField::Set(_)));
    assert!(matches!(command.images,PatchField::Set(ref images) if images.len()==2));
    let Call::Upsert(command) = &calls[2] else {
        panic!("upsert command")
    };
    assert!(command.title.is_some());
    assert!(matches!(command.price_estimate_max, PatchField::Set(_)));
    let Call::Withdraw(key) = &calls[3] else {
        panic!("withdraw key")
    };
    assert_eq!("full", key.source_listing_id.as_ref());
}
#[tokio::test]
async fn should_report_partial_and_all_failed_batches_without_losing_original_indices() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let value=call(&registry,name,json!({"listingSourceId":source,"products":[input_product(name,"same"),input_product(name,"fail"),input_product(name,"same")]}),context()).await.unwrap();
        assert_eq!("PARTIAL_SUCCESS", value["status"]);
        assert_eq!(2, value["succeededCount"]);
        assert_eq!(1, value["failedCount"]);
        assert_eq!(3, value["inputCount"]);
        assert_eq!(0, value["successes"][0]["index"]);
        assert_eq!(2, value["successes"][1]["index"]);
        assert_eq!(1, value["failures"][0]["index"]);
        assert_eq!("fail", value["failures"][0]["sourceListingId"]);
        let value = call(
            &registry,
            name,
            json!({"listingSourceId":source,"products":[input_product(name,"fail")]}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!("FAILED", value["status"]);
        assert_eq!(0, value["succeededCount"]);
        assert!(!value.to_string().contains("private provider"));
    }
    assert_eq!(16, items.calls.lock().unwrap().len());
}
#[tokio::test]
async fn should_keep_commit_failures_indexed_and_continue_without_exposing_causes() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let value = call(
            &registry,
            name,
            json!({"listingSourceId":source,"products":[input_product(name,"first"),input_product(name,"uncertain"),input_product(name,"last")]}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!("PARTIAL_SUCCESS", value["status"]);
        assert_eq!(2, value["succeededCount"]);
        assert_eq!(1, value["failedCount"]);
        assert_eq!(0, value["successes"][0]["index"]);
        assert_eq!(2, value["successes"][1]["index"]);
        assert_eq!(
            json!([{"index":1,"listingSourceId":source,"sourceListingId":"uncertain","error":"TEMPORARILY_UNAVAILABLE"}]),
            value["failures"]
        );
        assert!(!value.to_string().contains(PRIVATE_COMMIT_CAUSE));
    }
    assert_eq!(12, items.calls.lock().unwrap().len());
}
#[tokio::test]
async fn should_reject_later_conversion_failure_before_any_item_call_for_all_four_operations() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let mut invalid = input_product(name, "later");
        match name {
            "postPartnerProductListings" => invalid["title"]["text"] = json!("🌿✨"),
            "patchPartnerProductListings" => invalid["url"] = Value::Null,
            "putPartnerProductListings" => invalid["images"] = Value::Null,
            _ => invalid["sourceListingId"] = json!("   "),
        }
        let error = registry
            .get(name)
            .unwrap()
            .call(
                json!({"listingSourceId":source,"products":[input_product(name,"first"),invalid]}),
                context(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::InvalidArguments(_)));
    }
    assert!(items.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn should_reject_malformed_later_items_before_any_item_call_for_all_four_operations() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let mut invalid = input_product(name, "later");
        invalid["sourceListingId"] = json!(42);
        let error = registry
            .get(name)
            .unwrap()
            .call(
                json!({"listingSourceId":source,"products":[input_product(name,"first"),invalid]}),
                context(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::InvalidArguments(_)));
    }
    assert!(items.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn should_accept_authenticated_empty_and_one_hundred_and_reject_oversized_batches() {
    let (registry, items) = fixture();
    let source = ListingSourceId::new();
    for name in NAMES {
        let value = call(
            &registry,
            name,
            json!({"listingSourceId":source,"products":[]}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!("COMPLETED", value["status"]);
        assert_eq!(0, value["inputCount"]);
        let invalid =
            json!({"listingSourceId":source,"products":vec![input_product(name,"same");101]});
        let schema =
            serde_json::to_value(&*registry.get(name).unwrap().tool().input_schema).unwrap();
        assert!(
            !jsonschema::validator_for(&schema)
                .unwrap()
                .is_valid(&invalid)
        );
        assert!(matches!(
            registry.get(name).unwrap().call(invalid, context()).await,
            Err(OperationError::InvalidArguments(_))
        ));
        let value = call(
            &registry,
            name,
            json!({"listingSourceId":source,"products":vec![input_product(name,"same");100]}),
            context(),
        )
        .await
        .unwrap();
        assert_eq!(100, value["succeededCount"]);
    }
    assert_eq!(400, items.calls.lock().unwrap().len());
}

#[tokio::test]
async fn should_reject_a_later_duplicate_field_before_any_sync_write_through_raw_http() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
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
    for name in NAMES {
        let (registry, items) = fixture();
        let source = ListingSourceId::new();
        let valid = input_product(name, "valid");
        let duplicate = valid
            .to_string()
            .replacen("{", r#"{"sourceListingId":"ambiguous","#, 1);
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{{"listingSourceId":"{source}","products":[{valid},{duplicate}]}}}}}}"#
        );
        let app = crate::router(
            crate::TransportConfig {
                allowed_hosts: vec!["mcp.example.test".into()],
                ..Default::default()
            },
            registry,
            Arc::new(Gate),
        )
        .unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1")
                    .header("host", "mcp.example.test")
                    .header("content-type", "application/json")
                    .header("accept", "application/json, text/event-stream")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(StatusCode::OK, response.status());
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(-32602, response["error"]["code"], "{name}: {response}");
        assert!(items.calls.lock().unwrap().is_empty());
    }
}
