mod postgres;

use super::*;
use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    pagination::{Cursor, CursoredResult},
    patch_field::PatchField,
};
use domain_primitives::change_outcome::ChangeOutcome;
use listing_source_core::{ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use partnership_service::{
    ports::AdministeredListingSource, use_cases::queries::list_administered_listing_sources::*,
};
use product_listing_core::{
    description::Description,
    listing_lifecycle::ListingLifecycle,
    product_listing::{LotNumber, ProductListingAuction, ProductListingPricing},
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::{
    ports::{ListingSourceSummary, ProductListingStorageVersion, admin_product_listing_reader::*},
    use_cases::{
        commands::admin_product_listings::AdminProductListingMutationResult,
        commands::partner_product_listings::*,
        partner_product_listing_error::PartnerProductListingError,
        queries::partner_product_listings::*,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;
use url::Url;
#[derive(Debug, PartialEq)]
enum Call {
    Sources(OperationContext, ListAdministeredListingSourcesRequest),
    List(
        OperationContext,
        ListingSourceId,
        AdminProductListingSearchRequest,
    ),
    Get(OperationContext, ListingSourceId, ProductListingId),
    Update(OperationContext, UpdatePartnerProductListingCommand),
    Withdraw(OperationContext, PartnerProductListingLifecycleCommand),
    Restore(OperationContext, PartnerProductListingLifecycleCommand),
}
struct Fixture {
    calls: Mutex<Vec<Call>>,
    detail: AdminProductListingDetails,
    error: AtomicU8,
}
impl Fixture {
    fn new() -> Arc<Self> {
        let id = ProductListingId::new();
        let source = ListingSourceId::new();
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            error: AtomicU8::new(0),
            detail: AdminProductListingDetails {
                summary: AdminProductListingSummary {
                    product_listing_id: id,
                    product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                        "Original vase",
                        "a1b2c3",
                    )
                    .unwrap(),
                    source: ListingSourceSummary {
                        listing_source_id: source,
                        name: ListingSourceName::try_from("Original source").unwrap(),
                        slug_id: ListingSourceSlugId::raw("original-source").unwrap(),
                    },
                    source_listing_id: SourceListingId::try_from("SKU-42").unwrap(),
                    title: Some(Localized::new(Language::De, Title::from("Original vase"))),
                    pricing: ProductListingPricing::default(),
                    availability: None,
                    lifecycle: ListingLifecycle::Withdrawn,
                    auction_id: None,
                    version: ProductListingStorageVersion::try_from(i64::MAX).unwrap(),
                    created: OffsetDateTime::UNIX_EPOCH,
                    updated: OffsetDateTime::UNIX_EPOCH,
                },
                description: Some(Localized::new(
                    Language::De,
                    Description::from("Retained original description"),
                )),
                url: Url::parse("https://original.test/vase").unwrap(),
                images: std::iter::once(ProductListingImage::new(
                    Url::parse("https://original.test/private.jpg").unwrap(),
                ))
                .collect(),
                auction: ProductListingAuction::new(
                    None,
                    Some(LotNumber::try_from("42").unwrap()),
                    None,
                    None,
                    Some(OffsetDateTime::UNIX_EPOCH),
                    None,
                )
                .unwrap(),
                sale_observation: None,
            },
        })
    }
    fn services(self: &Arc<Self>) -> PartnerListingManagement {
        PartnerListingManagement {
            sources: self.clone(),
            get: self.clone(),
            list: self.clone(),
            update: self.clone(),
            withdraw: self.clone(),
            restore: self.clone(),
        }
    }
    fn outcome(&self) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        use product_listing_service::use_cases::admin_product_listing_error::AdminProductListingError as E;
        match self.error.load(Ordering::SeqCst) {
            1 => Err(PartnerProductListingError::Forbidden),
            2 => Err(E::ConcurrencyConflict.into()),
            3 => Err(E::NotFound.into()),
            4 => Err(E::TemporarilyUnavailable {
                source: application::error::static_error("PRIVATE_SENTINEL"),
            }
            .into()),
            _ => Ok(AdminProductListingMutationResult {
                product_listing_id: self.detail.summary.product_listing_id,
                expected_version: self.detail.summary.version,
                outcome: ChangeOutcome::Unchanged,
            }),
        }
    }
    fn scope(&self) -> Value {
        json!({"listingSourceId":self.detail.summary.source.listing_source_id,"productListingId":self.detail.summary.product_listing_id})
    }
}
#[async_trait::async_trait]
impl ListAdministeredListingSourcesUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdministeredListingSourcesRequest,
    ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Sources(context.clone(), request));
        let source = &self.detail.summary.source;
        Ok(ListAdministeredListingSourcesResult {
            items: vec![AdministeredListingSource {
                listing_source_id: source.listing_source_id,
                slug_id: source.slug_id.clone(),
                name: source.name.clone(),
            }],
            next: None,
        })
    }
}
#[async_trait::async_trait]
impl GetPartnerProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ProductListingId,
    ) -> Result<AdminProductListingDetails, PartnerProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Get(context.clone(), source, id));
        self.outcome()?;
        Ok(self.detail.clone())
    }
}
#[async_trait::async_trait]
impl ListPartnerProductListingsUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        request: AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, PartnerProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::List(context.clone(), source, request.clone()));
        self.outcome()?;
        Ok(CursoredResult {
            items: vec![self.detail.summary.clone()],
            cursor: Cursor {
                size: request.cursor.size,
                search_after: request.cursor.search_after.is_none().then_some(
                    AdminProductListingSearchCursor {
                        product_listing_id: self.detail.summary.product_listing_id,
                        created: OffsetDateTime::UNIX_EPOCH,
                        updated: OffsetDateTime::UNIX_EPOCH,
                        scope: request.scope,
                    },
                ),
            },
            total: None,
        })
    }
}
#[async_trait::async_trait]
impl UpdatePartnerProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartnerProductListingCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Update(context.clone(), command));
        self.outcome()
    }
}
#[async_trait::async_trait]
impl WithdrawPartnerProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Withdraw(context.clone(), command));
        self.outcome()
    }
}
#[async_trait::async_trait]
impl RestorePartnerProductListingUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: PartnerProductListingLifecycleCommand,
    ) -> Result<AdminProductListingMutationResult, PartnerProductListingError> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::Restore(context.clone(), command));
        self.outcome()
    }
}
fn context() -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::DelegatedUser {
                user_id: user_core::user_id::UserId::new(),
                capabilities: BTreeSet::from([
                    CredentialCapability::ListingSourcesRead,
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]),
            },
            request_id: RequestId::new("management"),
            correlation_id: CorrelationId::new("management"),
        },
        deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(10),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
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
        "input schema rejects {input}: {schema}"
    );
    let result = operation.call(input, context).await?;
    let output = result.structured_content.unwrap();
    let schema = serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
    assert!(
        jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&output),
        "output schema rejects {output}"
    );
    Ok(output)
}
#[tokio::test]
async fn should_expose_six_stable_operations_and_return_authoritative_retained_detail() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    let context = context();
    for (name, scope) in [
        (
            "getMyListingSources",
            CredentialCapability::ListingSourcesRead,
        ),
        (
            "listPartnerProductListings",
            CredentialCapability::ProductListingsRead,
        ),
        (
            "getPartnerProductListing",
            CredentialCapability::ProductListingsRead,
        ),
        (
            "updatePartnerProductListing",
            CredentialCapability::ProductListingsWrite,
        ),
        (
            "withdrawPartnerProductListing",
            CredentialCapability::ProductListingsWrite,
        ),
        (
            "restorePartnerProductListing",
            CredentialCapability::ProductListingsWrite,
        ),
    ] {
        let operation = registry.get(name).unwrap();
        assert_eq!(name, operation.name());
        assert!(operation.policy().access == AccessClass::Protected);
        assert_eq!(vec![scope], operation.policy().required_scopes);
    }
    let output = call(
        &registry,
        "getPartnerProductListing",
        fixture.scope(),
        context.clone(),
    )
    .await
    .unwrap();
    let expected = serde_json::to_value(DetailsData::from(fixture.detail.clone())).unwrap();
    assert_eq!(expected, output);
    assert_eq!("WITHDRAWN", output["lifecycle"]);
    assert_eq!(i64::MAX, output["expectedVersion"]);
    assert_eq!(
        "Retained original description",
        output["description"]["text"]
    );
    assert_eq!("https://original.test/private.jpg", output["images"][0]);
    assert_eq!("42", output["auction"]["lotNumber"]);
    let source = call(
        &registry,
        "getMyListingSources",
        json!({"size":100}),
        context.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        fixture.scope()["listingSourceId"],
        source["items"][0]["listingSourceId"]
    );
    assert!(source.get("next").is_none());
    assert!(
        matches!(&fixture.calls.lock().unwrap()[0],Call::Get(actual,_,_) if actual==&context.operation)
    );
}
#[tokio::test]
async fn should_map_every_query_filter_and_preserve_source_bound_search_cursor() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    let context = context();
    let mut input = json!({"listingSourceId":fixture.scope()["listingSourceId"],"query":" VASE ","sourceListingId":" SKU-42 ","lifecycle":"WITHDRAWN","availability":"RESERVED","auctionId":auction_core::AuctionId::new(),"sort":"created","order":"asc","size":100});
    let first = call(
        &registry,
        "listPartnerProductListings",
        input.clone(),
        context.clone(),
    )
    .await
    .unwrap();
    input["searchAfter"] = first["searchAfter"].clone();
    let second = call(
        &registry,
        "listPartnerProductListings",
        input.clone(),
        context.clone(),
    )
    .await
    .unwrap();
    assert!(second.get("searchAfter").is_none());
    let calls = fixture.calls.lock().unwrap();
    let Call::List(actor, source, request) = &calls[1] else {
        panic!("list call")
    };
    assert_eq!(&context.operation, actor);
    assert_eq!(Some(*source), request.scope.listing_source_id);
    assert_eq!(Some("vase"), request.scope.query.as_deref());
    assert_eq!(
        Some("SKU-42"),
        request.scope.source_listing_id.as_ref().map(|v| v.as_ref())
    );
    assert_eq!(100, request.cursor.size);
    assert_eq!(
        request.scope,
        request.cursor.search_after.as_ref().unwrap().scope
    );
    let expected: SearchQuery = serde_json::from_value(input).unwrap();
    assert_eq!(expected.into_request().unwrap(), *request);
}
#[tokio::test]
async fn should_preserve_full_versions_patch_values_and_lifecycle_outcomes() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    let context = context();
    let mut input = fixture.scope();
    input["expectedVersion"] = json!(i64::MAX);
    input["price"] = Value::Null;
    input["priceEstimateMin"] = json!({"currency":"EUR","amount":55});
    input["availability"] = Value::Null;
    input["images"] = json!([]);
    input["url"] = json!("https://new.test/vase");
    input["auction"] = json!({"auctionId":null,"lotNumber":"42","cataloguePosition":null,"timing":{"biddingOpens":null,"scheduledCloses":"2026-10-23T12:00:00Z"}});
    call(
        &registry,
        "updatePartnerProductListing",
        input.clone(),
        context.clone(),
    )
    .await
    .unwrap();
    let Call::Update(actor, command) = fixture.calls.lock().unwrap().remove(0) else {
        panic!("update")
    };
    assert_eq!(context.operation, actor);
    assert_eq!(i64::MAX as u64, u64::from(command.expected_version));
    input.as_object_mut().unwrap().remove("listingSourceId");
    input.as_object_mut().unwrap().remove("productListingId");
    let expected: UpdateData = serde_json::from_value(input).unwrap();
    assert_eq!(
        expected
            .into_command(command.product_listing_id)
            .unwrap()
            .patch,
        command.patch
    );
    assert!(matches!(command.patch.price, PatchField::Clear));
    assert!(matches!(command.patch.images,PatchField::Set(ref images) if images.is_empty()));
    assert!(matches!(
        command.patch.price_estimate_max,
        PatchField::Unchanged
    ));
    for name in [
        "withdrawPartnerProductListing",
        "restorePartnerProductListing",
    ] {
        let mut input = fixture.scope();
        input["expectedVersion"] = json!(i64::MAX);
        let result = call(&registry, name, input, context.clone()).await.unwrap();
        assert_eq!("UNCHANGED", result["outcome"]);
        assert_eq!(i64::MAX, result["expectedVersion"]);
    }
}
#[tokio::test]
async fn should_reject_malformed_inputs_and_preserve_safe_error_categories() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    for body in [
        json!({"userId":"spoof"}),
        json!({"size":0}),
        json!({"after":"garbage"}),
    ] {
        assert!(matches!(
            registry
                .get("getMyListingSources")
                .unwrap()
                .call(body, context())
                .await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    for patch in [
        json!({"expectedVersion":0}),
        json!({"expectedVersion":u64::MAX}),
        json!({"expectedVersion":1,"images":null}),
        json!({"expectedVersion":1,"auction":null}),
        json!({"expectedVersion":1,"extra":true}),
    ] {
        let mut input = fixture.scope();
        input
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(matches!(
            registry
                .get("updatePartnerProductListing")
                .unwrap()
                .call(input, context())
                .await,
            Err(OperationError::InvalidArguments(_))
        ));
    }
    assert!(fixture.calls.lock().unwrap().is_empty());
    for mode in 1..=4 {
        fixture.error.store(mode, Ordering::SeqCst);
        let result = registry
            .get("getPartnerProductListing")
            .unwrap()
            .call(fixture.scope(), context())
            .await;
        assert!(matches!(
            (mode, &result),
            (1, Err(OperationError::Forbidden))
                | (2, Err(OperationError::Conflict(None)))
                | (3, Err(OperationError::NotFound))
                | (4, Err(OperationError::Unavailable(_)))
        ));
        assert!(!format!("{}", result.unwrap_err()).contains("PRIVATE_SENTINEL"));
    }
}

#[test]
fn should_publish_strict_source_identity_schemas_for_every_listing_envelope() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    for name in [
        "listPartnerProductListings",
        "getPartnerProductListing",
        "updatePartnerProductListing",
        "withdrawPartnerProductListing",
        "restorePartnerProductListing",
    ] {
        let operation = registry.get(name).unwrap();
        let schema = serde_json::to_value(&*operation.tool().input_schema).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let mut input = fixture.scope();
        if name == "listPartnerProductListings" {
            input.as_object_mut().unwrap().remove("productListingId");
        }
        if [
            "updatePartnerProductListing",
            "withdrawPartnerProductListing",
            "restorePartnerProductListing",
        ]
        .contains(&name)
        {
            input["expectedVersion"] = json!(1);
        }
        assert!(validator.is_valid(&input));
        for invalid in [Value::Null, json!("bad"), json!(ProductListingId::new())] {
            input["listingSourceId"] = invalid;
            assert!(
                !validator.is_valid(&input),
                "{name} advertises invalid source {input}: {schema}"
            );
        }
        input.as_object_mut().unwrap().remove("listingSourceId");
        assert!(!validator.is_valid(&input), "{name} requires source");
    }
}

#[test]
fn should_require_nullable_authoritative_detail_members_in_output_schemas() {
    let fixture = Fixture::new();
    let registry = Registry::new(operations(fixture.services()).unwrap()).unwrap();
    let operation = registry.get("getPartnerProductListing").unwrap();
    let schema = serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let detail = serde_json::to_value(DetailsData::from(fixture.detail.clone())).unwrap();
    assert!(validator.is_valid(&detail));
    for field in [
        "title",
        "price",
        "priceEstimateMin",
        "priceEstimateMax",
        "availability",
        "auctionId",
        "description",
        "auction",
        "saleObservation",
    ] {
        let mut output = detail.clone();
        output[field] = Value::Null;
        assert!(validator.is_valid(&output), "{field} accepts null");
        output.as_object_mut().unwrap().remove(field);
        assert!(!validator.is_valid(&output), "{field} must be present");
    }
    for field in ["auctionId", "lotNumber", "cataloguePosition"] {
        let mut output = detail.clone();
        output["auction"][field] = Value::Null;
        assert!(validator.is_valid(&output), "{field} accepts null");
        output["auction"].as_object_mut().unwrap().remove(field);
        assert!(!validator.is_valid(&output), "{field} must be present");
    }
    for field in ["biddingOpens", "scheduledCloses", "reportedClosedAt"] {
        let mut output = detail.clone();
        output["auction"]["timing"][field] = Value::Null;
        assert!(validator.is_valid(&output), "{field} accepts null");
        output["auction"]["timing"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(!validator.is_valid(&output), "{field} must be present");
    }
}
