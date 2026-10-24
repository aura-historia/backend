//! Four synchronous integration operations; batch execution belongs to product-listing-service.
mod error;
use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::partner_product_listings::{
    CreateProductListingData, MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE,
    ProductListingBatchReportData, UpdateProductListingData, UpsertProductListingData,
    WithdrawProductListingData, prepare_batch,
};
use aura_historia_runtime::PartnerProductListingBatches;
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::commands::partner_product_listing_batches::PartnerProductListingBatchesUseCase;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BatchInput<T> {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(length(max = MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE))]
    products: Vec<T>,
}
fn policy(destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![CredentialCapability::ProductListingsWrite],
        read_only: false,
        destructive,
        idempotent: false,
        open_world: false,
    }
}

/// Family only; the partner release owns combined-registry publication.
pub fn operations(
    services: PartnerProductListingBatches,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        create_operation(services.batches.clone())?,
        update_operation(services.batches.clone())?,
        upsert_operation(services.batches.clone())?,
        withdraw_operation(services.batches)?,
    ])
}

fn create_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "postPartnerProductListings",
        "Create source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(false),
        move |input: BatchInput<CreateProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.create(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::create,
                ))
            }
        },
    )
}

fn update_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "patchPartnerProductListings",
        "Patch existing source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<UpdateProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.update(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::update,
                ))
            }
        },
    )
}

fn upsert_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "putPartnerProductListings",
        "Create, update or restore source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<UpsertProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.upsert(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::upsert,
                ))
            }
        },
    )
}

fn withdraw_operation(
    service: Arc<dyn PartnerProductListingBatchesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "deletePartnerProductListings",
        "Withdraw and retain source-keyed listings in original order, up to 100 items. All inputs are converted before the first mutation; duplicate sourceListingId values run sequentially. Each item commits independently and later service failures preserve earlier commits. Returns COMPLETED, PARTIAL_SUCCESS or FAILED with original indices, successful identities and per-item failures. Empty authenticated batches are valid. After an ambiguous timeout reconcile state before retrying; no automatic retries or batch idempotency.",
        policy(true),
        move |input: BatchInput<WithdrawProductListingData>, context| {
            let service = service.clone();
            async move {
                let prepared = prepare_batch(input.listing_source_id, input.products)
                    .map_err(|error| OperationError::InvalidArguments(Box::new(error)))?;
                let result = context
                    .execute(service.withdraw(&context.operation, prepared.commands))
                    .await?
                    .map_err(error::input)?;
                Ok(ProductListingBatchReportData::from_result(
                    prepared.source_listing_ids,
                    result,
                    error::withdraw,
                ))
            }
        },
    )
}

#[cfg(test)]
mod tests {
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
        let schema =
            serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
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
            let duplicate =
                valid
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

    #[cfg(test)]
    mod postgres {
        use super::*;
        use aura_historia_runtime::Runtime;
        use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
        const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
        async fn seed_source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
            let party = uuid::Uuid::now_v7();
            let source = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Operator')",
            )
            .bind(party)
            .bind(format!("party-{party}"))
            .execute(pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Partner source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
            (party, source)
        }
        async fn grant_source(
            pool: &sqlx::PgPool,
            user_id: user_core::user_id::UserId,
            party: uuid::Uuid,
            source: ListingSourceId,
        ) {
            let partnership = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(party)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
                .bind(partnership)
                .bind(user_id.into_uuid())
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
        }

        #[aura_integration_test(services=[SCHEMA], isolated=true)]
        async fn should_commit_items_independently_keep_duplicate_order_and_recheck_partner_authority()
         {
            let pool = get_postgres_client().await;
            let context = context();
            let Principal::DelegatedUser { user_id, .. } = context.operation.principal else {
                panic!("delegated actor")
            };
            sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
                .bind(user_id.into_uuid())
                .bind(format!("{user_id}@example.test"))
                .execute(&pool)
                .await
                .unwrap();
            let (party, source) = seed_source(&pool).await;
            grant_source(&pool, user_id, party, source).await;
            let runtime = Runtime::from_pool(pool.clone());
            let registry = Registry::new(
                operations(runtime.partner_product_listing_batches().clone()).unwrap(),
            )
            .unwrap();
            let product = |id: &str| json!({"sourceListingId":id,"title":{"language":"en","text":"Listing"},"url":"https://example.test/listing","images":[]});
            let created=call(&registry,NAMES[0],json!({"listingSourceId":source,"products":[product("same"),product("same"),product("after")]}),context.clone()).await.unwrap();
            assert_eq!("PARTIAL_SUCCESS", created["status"]);
            assert_eq!(2, created["succeededCount"]);
            assert_eq!(1, created["failures"][0]["index"]);
            assert_eq!("CONFLICT", created["failures"][0]["error"]);
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM product_listings WHERE listing_source_id=$1",
            )
            .bind(source.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(2, count);
            let patched=call(&registry,NAMES[1],json!({"listingSourceId":source,"products":[{"sourceListingId":"same","availability":"AVAILABLE"},{"sourceListingId":"missing","availability":"AVAILABLE"},{"sourceListingId":"same","availability":"SOLD_OUT"}]}),context.clone()).await.unwrap();
            assert_eq!("PARTIAL_SUCCESS", patched["status"]);
            assert_eq!(2, patched["succeededCount"]);
            assert_eq!(1, patched["failures"][0]["index"]);
            let availability:String=sqlx::query_scalar("SELECT availability FROM product_listings WHERE listing_source_id=$1 AND source_listing_id='same'").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!("SOLD_OUT", availability);
            let withdrawn=call(&registry,NAMES[3],json!({"listingSourceId":source,"products":[{"sourceListingId":"same"},{"sourceListingId":"missing"},{"sourceListingId":"same"}]}),context.clone()).await.unwrap();
            assert_eq!("PARTIAL_SUCCESS", withdrawn["status"]);
            assert_eq!(true, withdrawn["successes"][0]["changed"]);
            assert_eq!(false, withdrawn["successes"][1]["changed"]);
            let restored=call(&registry,NAMES[2],json!({"listingSourceId":source,"products":[{"sourceListingId":"same","availability":"AVAILABLE"},{"sourceListingId":"same","availability":"RESERVED"},product("new")]}),context.clone()).await.unwrap();
            assert_eq!("COMPLETED", restored["status"]);
            assert_eq!("UPDATED", restored["successes"][0]["outcome"]);
            assert_eq!("CREATED", restored["successes"][2]["outcome"]);
            let availability:String=sqlx::query_scalar("SELECT availability FROM product_listings WHERE listing_source_id=$1 AND source_listing_id='same'").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
            assert_eq!("RESERVED", availability);
            for name in NAMES {
                let mut unscoped = context.clone();
                unscoped.operation.principal = Principal::DelegatedUser {
                    user_id,
                    capabilities: BTreeSet::new(),
                };
                let report=call(&registry,name,json!({"listingSourceId":source,"products":[if name==NAMES[0] {product("forbidden")} else {json!({"sourceListingId":"same"})}]}),unscoped).await.unwrap();
                assert_eq!("FAILED", report["status"]);
                assert_eq!("FORBIDDEN", report["failures"][0]["error"]);
            }
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(source.into_uuid())
                .execute(&pool)
                .await
                .unwrap();
            for name in NAMES {
                let report=call(&registry,name,json!({"listingSourceId":source,"products":[if name==NAMES[0] {product("revoked")} else {json!({"sourceListingId":"same"})}]}),context.clone()).await.unwrap();
                assert_eq!("FAILED", report["status"]);
                assert_eq!("FORBIDDEN", report["failures"][0]["error"]);
            }
        }
    }
}
