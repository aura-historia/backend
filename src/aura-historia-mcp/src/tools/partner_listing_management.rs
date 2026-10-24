//! Partner management calls the same authoritative application handles as REST.
mod error;

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use application::operation_context::CredentialCapability;
use aura_historia_contracts::{
    listing_sources::administered::{
        AdministeredListingSourcesPageData, AdministeredListingSourcesQuery,
    },
    product_listing_management::{
        DetailsData, MutationData, SearchQuery, SearchResponse, UpdateData, VersionData, version,
    },
};
use aura_historia_runtime::PartnerListingManagement;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::commands::partner_product_listings::{
    PartnerProductListingLifecycleCommand, UpdatePartnerProductListingCommand,
};
use schemars::JsonSchema;
use serde::Deserialize;

use partnership_service::use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesUseCase;
use product_listing_service::use_cases::{
    commands::partner_product_listings::{
        RestorePartnerProductListingUseCase, UpdatePartnerProductListingUseCase,
        WithdrawPartnerProductListingUseCase,
    },
    queries::partner_product_listings::{
        GetPartnerProductListingUseCase, ListPartnerProductListingsUseCase,
    },
};
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
}
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(transform = require_source_schema)]
struct ListInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(flatten)]
    query: SearchQuery,
}
fn require_source_schema(schema: &mut schemars::Schema) {
    // The flattened shared query also supports an optional administrator source filter.
    // This envelope's typed, required path source owns that same member for partner calls.
    schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("derived listing input properties")
        .insert(
            "listingSourceId".into(),
            aura_historia_contracts::wire::schema::object_id(ListingSourceId::PREFIX)
                .as_value()
                .clone(),
        );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    #[serde(rename = "listingSourceId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(rename = "productListingId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    data: UpdateData,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LifecycleInput {
    #[serde(rename = "listingSourceId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(rename = "productListingId")]
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    data: VersionData,
}
impl LifecycleInput {
    fn into_command(self) -> Result<PartnerProductListingLifecycleCommand, OperationError> {
        Ok(PartnerProductListingLifecycleCommand {
            listing_source_id: self.listing_source_id,
            product_listing_id: self.product_listing_id,
            expected_version: version(self.data.expected_version).map_err(invalid)?,
        })
    }
}
fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> OperationError {
    OperationError::InvalidArguments(Box::new(error))
}
fn policy(scope: CredentialCapability, read_only: bool, destructive: bool) -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Protected,
        required_scopes: vec![scope],
        read_only,
        destructive,
        idempotent: true,
        open_world: false,
    }
}
/// Family only; the partner release owns the final combined registry.
pub fn operations(
    services: PartnerListingManagement,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        sources_operation(services.sources)?,
        list_operation(services.list)?,
        get_operation(services.get)?,
        update_operation(services.update)?,
        withdraw_operation(services.withdraw)?,
        restore_operation(services.restore)?,
    ])
}

fn sources_operation(
    service: Arc<dyn ListAdministeredListingSourcesUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getMyListingSources",
        "Page through your currently granted listing sources in stable name and ID order. Defaults to 50, maximum 100; pass next as after. Grants and delegated capability are checked on every page.",
        policy(CredentialCapability::ListingSourcesRead, true, false),
        move |input: AdministeredListingSourcesQuery, context| {
            let service = service.clone();
            async move {
                let request = input.into_request().map_err(invalid)?;
                context
                    .execute(service.execute(&context.operation, request))
                    .await?
                    .map(AdministeredListingSourcesPageData::from)
                    .map_err(error::sources)
            }
        },
    )
}

fn list_operation(
    service: Arc<dyn ListPartnerProductListingsUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "listPartnerProductListings",
        "Search authoritative listings within one granted source, including withdrawn listings. Preserves all query, sourceListingId, lifecycle, availability, auctionId, sort/order, size and searchAfter options. Cursors are bound to that source and normalized filters.",
        policy(CredentialCapability::ProductListingsRead, true, false),
        move |input: ListInput, context| {
            let service = service.clone();
            async move {
                let request = input
                    .query
                    .into_partner_request(input.listing_source_id)
                    .map_err(invalid)?;
                context
                    .execute(service.execute(&context.operation, input.listing_source_id, request))
                    .await?
                    .map(SearchResponse::from)
                    .map_err(error::management)
            }
        },
    )
}

fn get_operation(
    service: Arc<dyn GetPartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getPartnerProductListing",
        "Inspect authoritative original listing facts and expectedVersion within a currently granted source, including retained withdrawn listings. Original image URLs are privileged inspection data; previews must respect sensitivity policy.",
        policy(CredentialCapability::ProductListingsRead, true, false),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(
                        &context.operation,
                        input.listing_source_id,
                        input.product_listing_id,
                    ))
                    .await?
                    .map(DetailsData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn update_operation(
    service: Arc<dyn UpdatePartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "updatePartnerProductListing",
        "Patch an authoritative listing using its complete expectedVersion. Omitted fields preserve, optional nulls clear, and images: [] clears images. URL, images and the whole auction cannot be null. After a timeout, refetch and reconcile state/version before another command; mutations are never automatically retried.",
        policy(CredentialCapability::ProductListingsWrite, false, true),
        move |input: UpdateInput, context| {
            let service = service.clone();
            async move {
                let command = input
                    .data
                    .into_command(input.product_listing_id)
                    .map_err(invalid)?;
                let command = UpdatePartnerProductListingCommand {
                    listing_source_id: input.listing_source_id,
                    product_listing_id: command.product_listing_id,
                    expected_version: command.expected_version,
                    patch: command.patch,
                };
                context
                    .execute(service.execute(&context.operation, command))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn withdraw_operation(
    service: Arc<dyn WithdrawPartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "withdrawPartnerProductListing",
        "Withdraw and retain a listing using expectedVersion, returning the resulting identity, version and CHANGED/UNCHANGED outcome. A stale version conflicts. After a timeout, refetch and reconcile state/version before another command.",
        policy(CredentialCapability::ProductListingsWrite, false, true),
        move |input: LifecycleInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(&context.operation, input.into_command()?))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
            }
        },
    )
}

fn restore_operation(
    service: Arc<dyn RestorePartnerProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "restorePartnerProductListing",
        "Restore a retained listing to active with unknown availability using expectedVersion. A stale version conflicts; an already-active matching version is unchanged. After a timeout, refetch and reconcile state/version before another command.",
        policy(CredentialCapability::ProductListingsWrite, false, false),
        move |input: LifecycleInput, context| {
            let service = service.clone();
            async move {
                context
                    .execute(service.execute(&context.operation, input.into_command()?))
                    .await?
                    .map(MutationData::from)
                    .map_err(error::management)
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
        ports::{
            ListingSourceSummary, ProductListingStorageVersion, admin_product_listing_reader::*,
        },
        use_cases::{
            commands::admin_product_listings::AdminProductListingMutationResult,
            partner_product_listing_error::PartnerProductListingError,
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
        ) -> Result<ListAdministeredListingSourcesResult, ListAdministeredListingSourcesError>
        {
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
        let schema =
            serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
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
        let schema =
            serde_json::to_value(&**operation.tool().output_schema.as_ref().unwrap()).unwrap();
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
        async fn seed_listing(pool: &sqlx::PgPool, source: ListingSourceId) -> ProductListingId {
            use application::transaction::{Transaction, UnitOfWork};
            use platform_postgres::SqlxUnitOfWork;
            use product_listing_core::product_listing::{NewProductListing, ProductListing};
            use product_listing_postgres::{
                SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
            };
            use product_listing_service::ports::{
                ProductListingEventAppender, ProductListingEventAppenderFactory,
                ProductListingRepository, ProductListingRepositoryFactory,
                stamp_product_listing_event,
            };
            let id = ProductListingId::new();
            let mut listing = ProductListing::create(NewProductListing {
                id,
                title_slug_id: ProductListingSlugId::from_title_and_suffix(
                    &format!("Original {}", id.as_uuid()),
                    "a1b2c3",
                )
                .unwrap(),
                listing_source_id: source,
                source_listing_id: SourceListingId::try_from(id.to_string()).unwrap(),
                title: Some(Localized::new(Language::En, Title::from("Original"))),
                description: Some(Localized::new(Language::En, Description::from("Retained"))),
                pricing: ProductListingPricing::default(),
                availability: None,
                url: Url::parse("https://source.test/listing").unwrap(),
                images: std::iter::once(ProductListingImage::new(
                    Url::parse("https://source.test/original.jpg").unwrap(),
                ))
                .collect(),
                auction: None,
            })
            .unwrap();
            let event = stamp_product_listing_event(
                id,
                OffsetDateTime::now_utc(),
                listing.take_pending_event_payload().unwrap(),
            );
            let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
            SqlxProductListingRepositoryFactory::new()
                .in_transaction(&mut tx)
                .insert(&listing, event.event_id)
                .await
                .unwrap();
            SqlxProductListingEventAppenderFactory::new()
                .in_transaction(&mut tx)
                .append(&event)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            id
        }
        async fn error(
            registry: &Registry,
            name: &str,
            input: Value,
            context: InvocationContext,
        ) -> OperationError {
            registry
                .get(name)
                .unwrap()
                .call(input, context)
                .await
                .unwrap_err()
        }

        struct DatabaseFixture {
            pool: sqlx::PgPool,
            registry: Registry,
            context: InvocationContext,
            source: ListingSourceId,
            id: ProductListingId,
            foreign_id: ProductListingId,
            foreign_source: ListingSourceId,
            foreign_party: uuid::Uuid,
            user_id: user_core::user_id::UserId,
        }
        impl DatabaseFixture {
            fn target(&self) -> Value {
                json!({"listingSourceId":self.source,"productListingId":self.id})
            }
            fn lifecycle(&self, version: u64) -> Value {
                let mut input = self.target();
                input["expectedVersion"] = json!(version);
                input
            }
            fn listing_calls(&self) -> Vec<(&'static str, Value)> {
                vec![
                    (
                        "listPartnerProductListings",
                        json!({"listingSourceId":self.source}),
                    ),
                    ("getPartnerProductListing", self.target()),
                    ("updatePartnerProductListing", self.lifecycle(1)),
                    ("withdrawPartnerProductListing", self.lifecycle(1)),
                    ("restorePartnerProductListing", self.lifecycle(1)),
                ]
            }
            async fn assert_listing_calls_forbidden(&self, context: &InvocationContext) {
                for (name, input) in self.listing_calls() {
                    assert!(matches!(
                        error(&self.registry, name, input, context.clone()).await,
                        OperationError::Forbidden
                    ));
                }
            }
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

        async fn fixture() -> DatabaseFixture {
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
            let (foreign_party, foreign) = seed_source(&pool).await;
            grant_source(&pool, user_id, party, source).await;
            let id = seed_listing(&pool, source).await;
            seed_listing(&pool, source).await;
            let foreign_id = seed_listing(&pool, foreign).await;
            let runtime = Runtime::from_pool(pool.clone());
            let services = runtime.partner_listing_management();
            assert!(Arc::ptr_eq(
                &services.get,
                &runtime.partner_listing_management().get
            ));
            let registry = Registry::new(operations(services.clone()).unwrap()).unwrap();
            DatabaseFixture {
                pool,
                registry,
                context,
                source,
                id,
                foreign_id,
                foreign_source: foreign,
                foreign_party,
                user_id,
            }
        }

        #[aura_integration_test(services = [SCHEMA], isolated = true)]
        async fn should_keep_directory_continuations_and_listing_ids_within_the_authorized_source()
        {
            let f = fixture().await;
            let sources = call(
                &f.registry,
                "getMyListingSources",
                json!({"size":1}),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert_eq!(json!(f.source), sources["items"][0]["listingSourceId"]);
            assert!(sources.get("next").is_none());
            let page = call(
                &f.registry,
                "listPartnerProductListings",
                json!({"listingSourceId":f.source,"size":1}),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert!(page["searchAfter"].is_string());
            assert!(matches!(error(&f.registry,"listPartnerProductListings",json!({"listingSourceId":f.source,"size":1,"order":"asc","searchAfter":page["searchAfter"]}),f.context.clone()).await,OperationError::InvalidArguments(_)));
            for name in [
                "getPartnerProductListing",
                "updatePartnerProductListing",
                "withdrawPartnerProductListing",
                "restorePartnerProductListing",
            ] {
                let mut cross = json!({"listingSourceId":f.source,"productListingId":f.foreign_id});
                if name != "getPartnerProductListing" {
                    cross["expectedVersion"] = json!(1);
                }
                assert!(matches!(
                    error(&f.registry, name, cross, f.context.clone()).await,
                    OperationError::NotFound
                ));
            }
            grant_source(&f.pool, f.user_id, f.foreign_party, f.foreign_source).await;
            assert!(matches!(
                error(
                    &f.registry,
                    "listPartnerProductListings",
                    json!({"listingSourceId":f.foreign_source,"searchAfter":page["searchAfter"]}),
                    f.context.clone()
                )
                .await,
                OperationError::InvalidArguments(_)
            ));
        }

        #[aura_integration_test(services = [SCHEMA], isolated = true)]
        async fn should_preserve_authoritative_patches_version_conflicts_and_retained_lifecycle_facts()
         {
            let f = fixture().await;
            let detail = call(
                &f.registry,
                "getPartnerProductListing",
                f.target(),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert_eq!(1, detail["expectedVersion"]);
            assert_eq!("Retained", detail["description"]["text"]);
            let mut patch = f.lifecycle(1);
            patch["price"] = json!({"type":"ON_REQUEST"});
            patch["images"] = json!([]);
            let changed = call(
                &f.registry,
                "updatePartnerProductListing",
                patch.clone(),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert_eq!(2, changed["expectedVersion"]);
            assert_eq!("CHANGED", changed["outcome"]);
            assert!(matches!(
                error(
                    &f.registry,
                    "updatePartnerProductListing",
                    patch,
                    f.context.clone()
                )
                .await,
                OperationError::Conflict(None)
            ));
            let detail = call(
                &f.registry,
                "getPartnerProductListing",
                f.target(),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert_eq!("ON_REQUEST", detail["price"]["type"]);
            assert_eq!(json!([]), detail["images"]);
            assert_eq!(
                3,
                call(
                    &f.registry,
                    "withdrawPartnerProductListing",
                    f.lifecycle(2),
                    f.context.clone()
                )
                .await
                .unwrap()["expectedVersion"]
            );
            let retained = call(
                &f.registry,
                "getPartnerProductListing",
                f.target(),
                f.context.clone(),
            )
            .await
            .unwrap();
            assert_eq!("WITHDRAWN", retained["lifecycle"]);
            assert_eq!("Retained", retained["description"]["text"]);
            assert_eq!(
                4,
                call(
                    &f.registry,
                    "restorePartnerProductListing",
                    f.lifecycle(3),
                    f.context.clone()
                )
                .await
                .unwrap()["expectedVersion"]
            );
        }

        #[aura_integration_test(services = [SCHEMA], isolated = true)]
        async fn should_recheck_capabilities_and_current_grants_after_tool_discovery() {
            let f = fixture().await;
            let (listed, _) = f.registry.page(None).unwrap();
            assert_eq!(6, listed.len());
            let mut unscoped = f.context.clone();
            unscoped.operation.principal = Principal::DelegatedUser {
                user_id: f.user_id,
                capabilities: BTreeSet::new(),
            };
            f.assert_listing_calls_forbidden(&unscoped).await;
            assert!(matches!(
                error(&f.registry, "getMyListingSources", json!({}), unscoped).await,
                OperationError::Forbidden
            ));
            sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
                .bind(f.source.into_uuid())
                .execute(&f.pool)
                .await
                .unwrap();
            f.assert_listing_calls_forbidden(&f.context).await;
            assert_eq!(
                json!([]),
                call(
                    &f.registry,
                    "getMyListingSources",
                    json!({}),
                    f.context.clone()
                )
                .await
                .unwrap()["items"]
            );
        }
    }
}
