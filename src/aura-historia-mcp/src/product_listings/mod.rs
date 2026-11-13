//! Module-local public listing registrations. Release composition selects this family.
pub(crate) mod error;

use crate::{
    AccessClass, InvocationContext, OperationDefinition, OperationError, OperationPolicy,
    RegistryError,
};
use aura_historia_contracts::product_listings::{
    presentation::ProductListingPresentationData,
    product_data::{
        PersonalizedProductListingDetailsData, PersonalizedProductListingSummaryData,
        personalized_product_details_data, personalized_product_summary_data,
    },
    search::{
        CursoredProductListingsData, ProductListingSearchData, ProductListingSearchOptionsData,
    },
};
use aura_historia_runtime::PublicProductListings;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::{
    ports::{ProductListingEmbeddingLookup, SimilarProductListingsLimit},
    use_cases::{
        GetProductListingRequest, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, ProductListingLookup,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchInput {
    #[serde(flatten)]
    search: ProductListingSearchData,
    #[serde(flatten)]
    options: ProductListingSearchOptionsData,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    /// ID of the listing (`pl_…`).
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SimilarInput {
    /// ID of the listing (`pl_…`).
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
    /// Number of similar listings to return, from 1 to 50 (default 20); values outside that range are clamped.
    #[schemars(range(max = u64::MAX))]
    size: Option<u64>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlugInput {
    /// Slug ID of the listing, the last segment of an Aura Historia listing page URL.
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::slug")]
    product_listing_title_slug_id: ProductListingSlugId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
}
/// The REST pending response has no body. MCP needs an explicit result distinct from
/// a completed computation with zero neighbors.
#[derive(Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
enum SimilarOutput {
    EmbeddingPending,
    Ready {
        items: Vec<PersonalizedProductListingSummaryData>,
    },
}
fn policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
/// Build exactly the four existing listing operations without installing them globally.
pub fn operations(
    services: PublicProductListings,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        search(services.search)?,
        get(services.get.clone())?,
        get_by_slug(services.get)?,
        similar(services.similar)?,
    ])
}
fn search(
    service: std::sync::Arc<dyn product_listing_service::use_cases::SearchProductListingsUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "simpleSearchProductListings",
        "Search listings",
        "Search Aura Historia's index of art, antiques and design objects listed by dealers, auction houses, marketplaces and auction platforms. Use it to find listings by text and by filters such as price, dates, availability, provider, auction, location and shipping. Returns listing summaries in the requested language and currency, with the total when known; pass the returned searchAfter object unchanged to get the next page. For a signed-in user, each result also carries userState sections for which the token has the read scope: watchlist:read, search-filters:read, notifications:read or users:read.",
        policy(),
        move |input: SearchInput, context| {
            let service = service.clone();
            async move {
                let request = input
                    .options
                    .into_request(input.search)
                    .map_err(|error| OperationError::invalid_argument(error.field, error))?;
                let result = context
                    .execute(service.execute(&context.operation, request))
                    .await?
                    .map_err(error::search)?;
                Ok(CursoredProductListingsData::from(result))
            }
        },
    )
}
fn get(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListing",
        "Get listing",
        "Get the full details of one listing by its productListingId: text, prices, images, provider, availability, auction and shipping, in the requested language and currency. Use it when the user wants more than a search result shows. For a signed-in user, each result also carries userState sections for which the token has the read scope: watchlist:read, search-filters:read, notifications:read or users:read.",
        policy(),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                details(
                    service,
                    context,
                    ProductListingLookup::ById(input.product_listing_id),
                    input.presentation,
                )
                .await
            }
        },
    )
}
fn get_by_slug(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListingByTitleSlug",
        "Get listing by slug",
        "Get the full details of one listing by its productListingTitleSlugId, the last segment of an Aura Historia listing page URL (/products/{slug}). Returns the same details as the lookup by productListingId. For a signed-in user, each result also carries userState sections for which the token has the read scope: watchlist:read, search-filters:read, notifications:read or users:read.",
        policy(),
        move |input: SlugInput, context| {
            let service = service.clone();
            async move {
                details(
                    service,
                    context,
                    ProductListingLookup::ByTitleSlug(input.product_listing_title_slug_id),
                    input.presentation,
                )
                .await
            }
        },
    )
}
async fn details(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
    context: InvocationContext,
    lookup: ProductListingLookup,
    presentation: ProductListingPresentationData,
) -> Result<PersonalizedProductListingDetailsData, OperationError> {
    context
        .execute(service.execute(
            &context.operation,
            GetProductListingRequest {
                lookup,
                language: presentation.language,
                currency: presentation.currency,
            },
        ))
        .await?
        .map(personalized_product_details_data)
        .map_err(error::details)
}
fn similar(
    service: std::sync::Arc<
        dyn product_listing_service::use_cases::GetSimilarProductListingsUseCase,
    >,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getSimilarProductListings",
        "Find similar listings",
        "Find listings similar to one listing, identified by its productListingId. Returns up to `size` neighbours (default 20; values outside 1 to 50 are clamped) in the requested language and currency. EMBEDDING_PENDING means the listing is not yet prepared for comparison and a later call can succeed; READY with an empty list means nothing similar was found. For a signed-in user, each result also carries userState sections for which the token has the read scope: watchlist:read, search-filters:read, notifications:read or users:read.",
        policy(),
        move |input: SimilarInput, context| {
            let service = service.clone();
            async move {
                let result = context
                    .execute(service.execute(
                        &context.operation,
                        GetSimilarProductListingsRequest {
                            lookup: ProductListingEmbeddingLookup::ById(input.product_listing_id),
                            language: input.presentation.language,
                            currency: input.presentation.currency,
                            limit: input.size.map_or_else(
                                Default::default,
                                SimilarProductListingsLimit::clamped,
                            ),
                        },
                    ))
                    .await?
                    .map_err(error::similar)?;
                Ok(match result {
                    GetSimilarProductListingsResult::EmbeddingPending => {
                        SimilarOutput::EmbeddingPending
                    }
                    GetSimilarProductListingsResult::Ready(items) => SimilarOutput::Ready {
                        items: items
                            .into_iter()
                            .map(personalized_product_summary_data)
                            .collect(),
                    },
                })
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_RESPONSE_BYTES, Registry};
    use application::{
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
        pagination::{Cursor, CursoredResult},
        personalized::Personalized,
    };
    use fxrate_core::FxRateId;
    use listing_source_core::{ListingSourceName, ListingSourceSlugId};
    use localization::{Language, Localized};
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing::ProductListingPricing, source_listing_id::SourceListingId, title::Title,
    };
    use product_listing_service::listing_text::ListingText;
    use product_listing_service::{ports::ListingSourceSummary, use_cases::*};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct Calls {
        search: Vec<(OperationContext, SearchProductListingsRequest)>,
        details: Vec<(OperationContext, GetProductListingRequest)>,
        similar: Vec<(OperationContext, GetSimilarProductListingsRequest)>,
    }
    struct FakeServices {
        calls: Arc<Mutex<Calls>>,
        pending: bool,
        empty: bool,
    }
    #[async_trait::async_trait]
    impl SearchProductListingsUseCase for FakeServices {
        async fn execute(
            &self,
            context: &OperationContext,
            request: SearchProductListingsRequest,
        ) -> Result<SearchProductListingsResult, SearchProductListingsError> {
            self.calls
                .lock()
                .unwrap()
                .search
                .push((context.clone(), request.clone()));
            if request
                .cursor
                .as_ref()
                .and_then(|cursor| cursor.search_after.as_ref())
                .is_some_and(|cursor| !cursor.search_after.is_array())
            {
                return Err(SearchProductListingsError::InvalidCursor);
            }
            Ok(CursoredResult {
                items: vec![summary()],
                total: Some(42),
                cursor: Cursor {
                    size: 1,
                    search_after: request.cursor.and_then(|cursor| cursor.search_after),
                },
            })
        }
    }
    #[async_trait::async_trait]
    impl GetProductListingUseCase for FakeServices {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetProductListingRequest,
        ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError> {
            let withdrawn = request.lookup
                == ProductListingLookup::ById("pl_01k6k0w000e008000000000009".parse().unwrap());
            self.calls
                .lock()
                .unwrap()
                .details
                .push((context.clone(), request));
            if withdrawn {
                return Err(GetProductListingError::NotFound);
            }
            Ok(details_view())
        }
    }
    #[async_trait::async_trait]
    impl GetSimilarProductListingsUseCase for FakeServices {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetSimilarProductListingsRequest,
        ) -> Result<GetSimilarProductListingsResult, GetSimilarProductListingsError> {
            self.calls
                .lock()
                .unwrap()
                .similar
                .push((context.clone(), request));
            Ok(if self.pending {
                GetSimilarProductListingsResult::EmbeddingPending
            } else {
                GetSimilarProductListingsResult::Ready(if self.empty {
                    vec![]
                } else {
                    vec![summary()]
                })
            })
        }
    }
    fn setup(pending: bool, empty: bool) -> (Registry, Arc<Mutex<Calls>>) {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let services = Arc::new(FakeServices {
            calls: calls.clone(),
            pending,
            empty,
        });
        let handles = PublicProductListings {
            get: services.clone(),
            similar: services.clone(),
            search: services,
        };
        (Registry::new(operations(handles).unwrap()).unwrap(), calls)
    }
    fn context() -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("listing-test"),
                correlation_id: CorrelationId::new("listing-test"),
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            cancellation: CancellationToken::new(),
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../aura-historia-contracts/tests/fixtures/listing-search.json"
        ))
        .unwrap()
    }
    async fn call(registry: &Registry, name: &str, input: Value) -> Result<Value, OperationError> {
        let operation = registry.get(name).unwrap();
        let schema = serde_json::to_value(&operation.tool().input_schema).unwrap();
        assert!(
            jsonschema::is_valid(&schema, &input),
            "input schema rejects fixture {input}"
        );
        let response = operation.call(input, context()).await?;
        let value = response.structured_content.unwrap();
        let schema =
            serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            validator.is_valid(&value),
            "{}",
            validator
                .iter_errors(&value)
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        Ok(value)
    }
    #[test]
    fn should_register_exactly_the_four_public_operation_ids_with_complete_search_properties() {
        let (registry, _) = setup(false, false);
        let names = [
            "simpleSearchProductListings",
            "getProductListing",
            "getProductListingByTitleSlug",
            "getSimilarProductListings",
        ];
        for name in names {
            let operation = registry.get(name).unwrap();
            assert_eq!(name, operation.name());
            let policy = operation.policy();
            assert_eq!(AccessClass::Public, policy.access);
            assert!(policy.required_scopes.is_empty());
            assert!(
                policy.read_only && policy.idempotent && !policy.destructive && !policy.open_world
            );
        }
        let properties = registry
            .get("simpleSearchProductListings")
            .unwrap()
            .tool()
            .input_schema["properties"]
            .as_object()
            .unwrap();
        for field in fixture().as_object().unwrap().keys() {
            assert!(
                properties.contains_key(field),
                "missing input field {field}"
            );
        }
    }
    #[tokio::test]
    async fn should_map_every_search_field_and_preserve_full_width_fx_continuation() {
        let (registry, calls) = setup(false, false);
        let input = fixture();
        let expected = serde_json::from_value::<ProductListingSearchOptionsData>(input.clone())
            .unwrap()
            .into_request(serde_json::from_value(input.clone()).unwrap())
            .unwrap();
        let output = call(&registry, "simpleSearchProductListings", input.clone())
            .await
            .unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(1, calls.search.len());
        assert!(matches!(calls.search[0].0.principal, Principal::Anonymous));
        assert_eq!(expected, calls.search[0].1);
        assert_eq!(100, expected.cursor.as_ref().unwrap().size);
        assert_eq!(Currency::Jpy, expected.search.currency);
        assert_eq!(Language::De, expected.search.language);
        assert_eq!(
            Some(u64::MAX),
            expected.search.price_query.unwrap().max.map(u64::from)
        );
        assert_eq!(
            Some(i64::MAX as u64),
            expected
                .search
                .shipping
                .as_ref()
                .unwrap()
                .maximum_per_item()
                .map(|p| u64::from(p.monetary_amount))
        );
        assert_eq!(input["searchAfter"], output["searchAfter"]);
        assert_eq!(42, output["total"]);
        assert_eq!(1, output["size"]);
        assert!(output["items"][0].get("userState").is_none());
    }
    #[tokio::test]
    async fn should_keep_both_detail_lookups_and_shared_payload_redaction() {
        let (registry, calls) = setup(false, false);
        let id = ProductListingId::new();
        let slug = ProductListingSlugId::raw("cabinet-abcdef").unwrap();
        for (name, input) in [
            (
                "getProductListing",
                json!({"productListingId":id,"language":"de-CH","currency":"JPY"}),
            ),
            (
                "getProductListingByTitleSlug",
                json!({"productListingTitleSlugId":slug,"language":"de-CH","currency":"JPY"}),
            ),
        ] {
            let output = call(&registry, name, input).await.unwrap();
            assert_eq!(
                serde_json::to_value(personalized_product_details_data(details_view())).unwrap(),
                output
            );
            assert!(output.get("userState").is_none());
            assert!(output["item"]["images"][0]["url"].is_null());
            assert_eq!(
                "REQUIRES_CONSENT",
                output["item"]["contentPolicy"]["decision"]
            );
            assert_eq!("merchant:local/42", output["item"]["sourceListingId"]);
            assert_eq!("ACTIVE", output["item"]["lifecycle"]);
            assert_eq!(
                u64::MAX,
                output["item"]["pricing"]["source"]["price"]["amount"]
            );
        }
        let calls = calls.lock().unwrap();
        assert_eq!(ProductListingLookup::ById(id), calls.details[0].1.lookup);
        assert_eq!(
            ProductListingLookup::ByTitleSlug(slug),
            calls.details[1].1.lookup
        );
        for (context, request) in &calls.details {
            assert!(matches!(context.principal, Principal::Anonymous));
            assert_eq!(Language::De, request.language);
            assert_eq!(Currency::Jpy, request.currency);
        }
    }
    #[tokio::test]
    async fn should_preserve_withdrawn_listing_not_found_from_the_service() {
        let (registry, _) = setup(false, false);
        let result = registry
            .get("getProductListing")
            .unwrap()
            .call(
                json!({"productListingId":"pl_01k6k0w000e008000000000009"}),
                context(),
            )
            .await;
        assert!(matches!(result, Err(OperationError::NotFound(_))));
    }
    #[tokio::test]
    async fn should_distinguish_pending_similarity_from_ready_empty() {
        let id = ProductListingId::new();
        let (ready, _) = setup(false, false);
        let output = call(
            &ready,
            "getSimilarProductListings",
            json!({"productListingId":id}),
        )
        .await
        .unwrap();
        assert_eq!("READY", output["status"]);
        assert_eq!(
            serde_json::to_value(personalized_product_summary_data(summary())).unwrap(),
            output["items"][0]
        );
        for (pending, empty, expected) in [
            (true, false, json!({"status":"EMBEDDING_PENDING"})),
            (false, true, json!({"status":"READY","items":[]})),
        ] {
            let (registry, calls) = setup(pending, empty);
            assert_eq!(
                expected,
                call(
                    &registry,
                    "getSimilarProductListings",
                    json!({"productListingId":id})
                )
                .await
                .unwrap()
            );
            {
                let calls = calls.lock().unwrap();
                assert_eq!(1, calls.similar.len());
                assert_eq!(
                    ProductListingEmbeddingLookup::ById(id),
                    calls.similar[0].1.lookup
                );
                assert_eq!(Language::En, calls.similar[0].1.language);
                assert_eq!(Currency::Eur, calls.similar[0].1.currency);
                assert!(matches!(calls.similar[0].0.principal, Principal::Anonymous));
            }
        }
    }
    #[tokio::test]
    async fn should_request_twenty_neighbours_unless_a_size_is_given() {
        let (registry, calls) = setup(false, false);

        call(
            &registry,
            "getSimilarProductListings",
            json!({"productListingId":ProductListingId::new()}),
        )
        .await
        .unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            SimilarProductListingsLimit::default(),
            calls.similar[0].1.limit
        );
        assert_eq!(20, calls.similar[0].1.limit.get());
    }
    #[tokio::test]
    async fn should_pass_the_requested_size_and_clamp_it_to_one_through_fifty() {
        for (requested, expected) in [(1, 1), (20, 20), (50, 50), (0, 1), (51, 50), (u64::MAX, 50)]
        {
            let (registry, calls) = setup(false, false);

            call(
                &registry,
                "getSimilarProductListings",
                json!({"productListingId":ProductListingId::new(),"size":requested}),
            )
            .await
            .unwrap();

            assert_eq!(
                expected,
                calls.lock().unwrap().similar[0].1.limit.get(),
                "size {requested}"
            );
        }
    }
    #[tokio::test]
    async fn should_reject_a_size_that_is_not_a_non_negative_integer() {
        let (registry, calls) = setup(false, false);
        let op = registry.get("getSimilarProductListings").unwrap();

        for size in [json!(-1), json!(1.5), json!("5"), json!(null)] {
            let result = op
                .call(
                    json!({"productListingId":ProductListingId::new(),"size":size}),
                    context(),
                )
                .await;
            // A null size means "not given"; every other value is not an unsigned integer.
            let rejected = matches!(result, Err(OperationError::InvalidArguments(_)));
            assert_eq!(!size.is_null(), rejected, "size {size}");
        }
        assert_eq!(1, calls.lock().unwrap().similar.len());
    }
    #[tokio::test]
    async fn should_name_the_rejected_argument_and_reason_in_the_invalid_params_message() {
        let (registry, _) = setup(false, false);
        let id = ProductListingId::new();
        for (name, input, expected) in [
            (
                "getProductListing",
                json!({}),
                "missing field `productListingId`",
            ),
            (
                "getProductListing",
                json!({"productListingId":id,"bogus":1}),
                "unknown field `bogus`",
            ),
            (
                "getProductListing",
                json!({"productListingId":"pl_bad"}),
                "productListingId: malformed object ID",
            ),
            (
                "getSimilarProductListings",
                json!({"productListingId":id,"size":-1}),
                "size: invalid value, expected u64",
            ),
            (
                "simpleSearchProductListings",
                json!({"excludeProductListingId":["pl_bad"]}),
                "excludeProductListingId: must be a valid ProductListing ID",
            ),
            (
                "simpleSearchProductListings",
                json!({"sort":"price"}),
                "sort: Expected any of: 'score', 'updated', 'created'. Got: 'price'",
            ),
        ] {
            let error = registry
                .get(name)
                .unwrap()
                .call(input.clone(), context())
                .await
                .unwrap_err();

            let rejection = error.into_protocol().unwrap_err();

            assert_eq!(-32602, rejection.code.0, "{name} {input}");
            assert_eq!(
                format!("Invalid tool arguments: {expected}"),
                rejection.message,
                "{name} {input}"
            );
        }
    }
    #[tokio::test]
    async fn should_never_repeat_a_supplied_value_in_the_invalid_params_message() {
        let (registry, _) = setup(false, false);
        let id = ProductListingId::new();
        for (name, input, secret) in [
            (
                "getProductListing",
                json!({"productListingId":"private-malformed"}),
                "private-malformed",
            ),
            (
                "getProductListing",
                json!({"productListingId":id,"language":"private-code"}),
                "private-code",
            ),
            (
                "getSimilarProductListings",
                json!({"productListingId":id,"size":"private-size"}),
                "private-size",
            ),
            (
                "simpleSearchProductListings",
                json!({"price":{"min":"private-price"}}),
                "private-price",
            ),
            (
                "simpleSearchProductListings",
                json!({"created":{"min":"private-date"}}),
                "private-date",
            ),
        ] {
            let error = registry
                .get(name)
                .unwrap()
                .call(input, context())
                .await
                .unwrap_err();

            let message = error.into_protocol().unwrap_err().message;

            assert!(message.starts_with("Invalid tool arguments: "), "{message}");
            assert!(!message.contains(secret), "{name}: {message}");
        }
    }
    #[test]
    fn should_say_which_listing_was_not_found_and_what_to_check() {
        let texts =
            error::details(product_listing_service::use_cases::GetProductListingError::NotFound)
                .tool_error_texts();

        assert_eq!("NOT_FOUND", texts[0]);
        assert!(texts[1].starts_with("Listing not found"), "{texts:?}");
        assert!(texts[1].contains("productListingId"), "{texts:?}");
    }
    #[test]
    fn should_describe_the_similarity_size_range_default_and_clamping() {
        let (registry, _) = setup(false, false);
        let tool = registry.get("getSimilarProductListings").unwrap().tool();

        let description = tool.input_schema["properties"]["size"]["description"]
            .as_str()
            .unwrap();

        for fact in ["1 to 50", "default 20", "clamped"] {
            assert!(description.contains(fact), "{fact}: {description}");
        }
        assert!(
            tool.description
                .as_deref()
                .unwrap()
                .contains("up to `size` neighbours")
        );
    }
    #[tokio::test]
    async fn should_reject_invalid_identifiers_enums_ranges_sort_and_cursor() {
        let (registry, calls) = setup(false, false);
        for input in [
            json!({"listingSourceId":["pl_bad"]}),
            json!({"availability":["UNKNOWN"]}),
            json!({"orderability":["UNKNOWN"]}),
            json!({"price":{"min":-1}}),
            json!({"created":{"min":"yesterday"}}),
            json!({"sort":"price"}),
            json!({"order":"asc"}),
            json!({"size":-1}),
            json!({"searchAfter":{"fxRateId":"fx_bad","searchAfter":[]}}),
        ] {
            assert!(matches!(
                registry
                    .get("simpleSearchProductListings")
                    .unwrap()
                    .call(input, context())
                    .await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert!(calls.lock().unwrap().search.is_empty());
        let invalid = json!({"searchAfter":{"fxRateId":FxRateId::new(),"searchAfter":"bad"}});
        assert!(matches!(
            registry
                .get("simpleSearchProductListings")
                .unwrap()
                .call(invalid, context())
                .await,
            Err(OperationError::InvalidArguments(_))
        ));
        for input in [
            json!({"productListingId":"pl_bad"}),
            json!({"productListingId":ProductListingId::new().as_uuid().to_string()}),
        ] {
            assert!(matches!(
                registry
                    .get("getProductListing")
                    .unwrap()
                    .call(input, context())
                    .await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
    }
    #[tokio::test]
    async fn should_cancel_before_invoking_services_and_preserve_safe_error_causes() {
        let (registry, calls) = setup(false, false);
        let invocation = context();
        invocation.cancellation.cancel();
        assert!(matches!(
            registry
                .get("getProductListing")
                .unwrap()
                .call(
                    json!({"productListingId":ProductListingId::new()}),
                    invocation
                )
                .await,
            Err(OperationError::Unavailable(_))
        ));
        assert!(calls.lock().unwrap().details.is_empty());
        assert!(matches!(
            error::details(GetProductListingError::Shipping(
                product_listing_service::shipping::ShippingReadError::Assessment(
                    shipping_service::assessment::AssessmentError::Forbidden
                )
            )),
            OperationError::Forbidden
        ));
        let error = error::details(GetProductListingError::BeginTransactionFailed(
            application::error::static_error("private diagnostic"),
        ));
        assert!(matches!(error, OperationError::Unavailable(_)));
        use std::error::Error;
        assert_eq!(
            "private diagnostic",
            error.source().unwrap().source().unwrap().to_string()
        );
        assert!(matches!(
            error::details(GetProductListingError::NotFound),
            OperationError::NotFound(_)
        ));
    }
    #[test]
    fn should_keep_product_search_and_similarity_sources_out_of_protocol_errors() {
        use std::error::Error;
        let unavailable = error::search(
            SearchProductListingsError::ProductListingSearchQueryFailed {
                source: application::error::static_error("PRIVATE_OPENSEARCH_SENTINEL"),
            },
        );
        assert!(matches!(unavailable, OperationError::Unavailable(_)));
        assert_eq!(
            "PRIVATE_OPENSEARCH_SENTINEL",
            unavailable.source().unwrap().source().unwrap().to_string()
        );
        let response = format!("{:?}", unavailable.into_protocol());
        assert!(response.contains("TEMPORARILY_UNAVAILABLE"));
        assert!(!response.contains("PRIVATE_OPENSEARCH_SENTINEL"));

        let internal = error::search(
            SearchProductListingsError::ProductListingSearchReadModelInvalid {
                source: application::error::static_error("PRIVATE_OPENSEARCH_SENTINEL"),
            },
        );
        assert!(matches!(internal, OperationError::Internal(_)));
        let response = format!("{:?}", internal.into_protocol());
        assert!(response.contains("Operation failed"));
        assert!(!response.contains("PRIVATE_OPENSEARCH_SENTINEL"));

        let similar_unavailable = error::similar(
            GetSimilarProductListingsError::SimilaritySearchUnavailable {
                source: application::error::static_error("PRIVATE_OPENSEARCH_SENTINEL"),
            },
        );
        assert!(matches!(
            similar_unavailable,
            OperationError::Unavailable(_)
        ));
        assert_eq!(
            "PRIVATE_OPENSEARCH_SENTINEL",
            similar_unavailable
                .source()
                .unwrap()
                .source()
                .unwrap()
                .to_string()
        );
        let response = format!("{:?}", similar_unavailable.into_protocol());
        assert!(response.contains("TEMPORARILY_UNAVAILABLE"));
        assert!(!response.contains("PRIVATE_OPENSEARCH_SENTINEL"));
    }
    fn source() -> ListingSourceSummary {
        ListingSourceSummary {
            listing_source_id: "ls_01k6k0w000e008000000000002".parse().unwrap(),
            name: ListingSourceName::try_from("Source").unwrap(),
            slug_id: ListingSourceSlugId::raw("source").unwrap(),
            operator: product_listing_service::ports::ListingSourceOperatorSummary {
                name: party_core::party_name::PartyName::try_from("Operator").unwrap(),
                partner: false,
            },
        }
    }
    fn details_view() -> PersonalizedProductListingDetailsView {
        Personalized {
            item: ProductListingDetailsView {
                shipping: None,
                product_listing_id: "pl_01k6k0w000e008000000000001".parse().unwrap(),
                product_listing_title_slug_id: None,
                event_id: "evt_01k6k0w000e008000000000007".parse().unwrap(),
                source: source(),
                source_listing_id: SourceListingId::try_from("merchant:local/42").unwrap(),
                title: Some(ListingText::localized(Localized::new(
                    Language::En,
                    Title::from("Cabinet"),
                ))),
                description: None,
                pricing: ProductListingPricingPresentation {
                    source: ProductListingPricing {
                        price: Some(
                            Price::new(MonetaryAmount::from(u64::MAX), Currency::Eur).into(),
                        ),
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    display: DisplayProductListingPricing {
                        price: Some(
                            Price::new(MonetaryAmount::from(u64::MAX), Currency::Jpy).into(),
                        ),
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    valuation: ProductListingPricingValuation::Current {
                        fx_rate_id: "fx_01k6k0w000e008000000000005".parse().unwrap(),
                        captured_at: OffsetDateTime::UNIX_EPOCH,
                    },
                },
                availability: Some(ListingAvailability::InStock),
                lifecycle: ListingLifecycle::Active,
                url: "https://source.example/product/42?ref=aura"
                    .parse()
                    .unwrap(),
                view_link: listing_source_core::ViewLink::non_affiliate(
                    "https://aura.example/product-listings/pl_01k6k0w000e008000000000001"
                        .parse()
                        .unwrap(),
                ),
                images: vec![ProductListingImageView { url: None }],
                content_policy: Some(
                    product_listing_core::content_policy::ContentPolicyDecision::RequiresConsent(
                        product_listing_core::content_policy::SensitiveContentCategory::NaziGermany,
                    ),
                ),
                auction: None,
                lot: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        }
    }
    fn summary() -> PersonalizedProductListingSummary {
        let view = details_view().item;
        Personalized {
            item: ProductListingSummary {
                shipping: None,
                geography: None,
                product_listing_id: view.product_listing_id,
                product_listing_title_slug_id: view.product_listing_title_slug_id,
                event_id: view.event_id,
                source: view.source,
                source_listing_id: view.source_listing_id,
                auction_id: None,
                title: view.title.map(|title| title.localized),
                display_price: view.pricing.display.price,
                price_valuation: ProductListingSummaryPriceValuation::Current {
                    fx_rate_id: "fx_01k6k0w000e008000000000005".parse().unwrap(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
                availability: view.availability,
                lifecycle: view.lifecycle,
                url: view.url,
                view_link: view.view_link,
                image: view.images.into_iter().next(),
                content_policy: view.content_policy,
                updated: view.updated,
            },
            user_state: None,
        }
    }
}
