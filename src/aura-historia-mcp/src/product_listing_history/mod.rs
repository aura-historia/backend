//! Complete public listing history registration, selected by release composition.

use crate::{AccessClass, OperationDefinition, OperationError, OperationPolicy, RegistryError};
use aura_historia_contracts::product_listings::history::{
    ProductListingHistoryEntryData, ProductListingHistoryQueryData,
};
use aura_historia_runtime::PublicProductListingHistory;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::use_cases::{
    GetProductListingHistoryError, GetProductListingHistoryRequest, ProductListingHistoryLookup,
};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    query: ProductListingHistoryQueryData,
}

/// Register the existing complete-history operation without installing it globally.
pub fn operations(
    services: PublicProductListingHistory,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![OperationDefinition::new(
        "getProductListingHistory",
        "Get the complete public listing history in original event order, with optional per-event historical display currency. Returns every event in one call; oversized results fail explicitly.",
        OperationPolicy {
            access: AccessClass::Public,
            required_scopes: vec![],
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        move |input: HistoryInput, context| {
            let service = services.get.clone();
            async move {
                context
                    .execute(service.execute(
                        &context.operation,
                        GetProductListingHistoryRequest {
                            lookup: ProductListingHistoryLookup::ById(input.product_listing_id),
                            currency: input.query.currency,
                        },
                    ))
                    .await?
                    .map(|entries| {
                        entries
                            .into_iter()
                            .map(ProductListingHistoryEntryData::from)
                            .collect::<Vec<_>>()
                    })
                    .map_err(history_error)
            }
        },
    )?])
}

fn history_error(error: GetProductListingHistoryError) -> OperationError {
    use GetProductListingHistoryError::*;
    match error {
        NotFound => OperationError::NotFound,
        error @ (QueryFailed { .. }
        | FxSnapshotUnavailable { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_)) => OperationError::Unavailable(Box::new(error)),
        error @ (InvalidReadModel { .. } | PriceConversionFailed(_)) => {
            OperationError::Internal(Box::new(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InvocationContext, MAX_RESPONSE_BYTES, Registry};
    use application::{
        error::static_error,
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    };
    use fxrate_core::FxRateId;
    use listing_source_core::ListingSourceId;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        product_listing_price::ProductListingPrice, source_listing_id::SourceListingId,
    };
    use product_listing_service::use_cases::{
        GetProductListingHistoryUseCase, ProductListingDiscoveryHistory,
        ProductListingHistoryChange, ProductListingHistoryEntry, ProductListingHistoryEntryKind,
        ProductListingHistoryPrice, ProductListingHistoryPriceDisplay,
        ProductListingHistoryPricing,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use time::{Duration, OffsetDateTime};
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    enum Outcome {
        History(Vec<ProductListingHistoryEntry>),
        NotFound,
        Pending,
    }
    struct FakeHistory {
        outcome: Outcome,
        calls: Mutex<Vec<(OperationContext, GetProductListingHistoryRequest)>>,
    }
    #[async_trait::async_trait]
    impl GetProductListingHistoryUseCase for FakeHistory {
        async fn execute(
            &self,
            context: &OperationContext,
            request: GetProductListingHistoryRequest,
        ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError> {
            self.calls.lock().unwrap().push((context.clone(), request));
            match &self.outcome {
                Outcome::History(entries) => Ok(entries.clone()),
                Outcome::NotFound => Err(GetProductListingHistoryError::NotFound),
                Outcome::Pending => std::future::pending().await,
            }
        }
    }
    fn setup(outcome: Outcome) -> (Registry, Arc<FakeHistory>) {
        let service = Arc::new(FakeHistory {
            outcome,
            calls: Mutex::new(vec![]),
        });
        let registry = Registry::new(
            operations(PublicProductListingHistory {
                get: service.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        (registry, service)
    }
    fn context() -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("history-request"),
                correlation_id: CorrelationId::new("history-correlation"),
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            cancellation: CancellationToken::new(),
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }
    fn operation(registry: &Registry) -> &OperationDefinition {
        registry.get("getProductListingHistory").unwrap()
    }
    fn input(product_listing_id: ProductListingId) -> Value {
        json!({"productListingId":product_listing_id,"currency":"USD"})
    }
    fn entries(
        product_listing_id: ProductListingId,
        count: usize,
    ) -> Vec<ProductListingHistoryEntry> {
        let first_fx = FxRateId::new();
        let later_fx = FxRateId::new();
        (0..count)
            .map(|index| {
                let captured_at =
                    OffsetDateTime::UNIX_EPOCH + Duration::seconds(if index < 2 { -60 } else { 1 });
                let price = ProductListingHistoryPrice {
                    source: ProductListingPrice::Monetary(Price::new(
                        MonetaryAmount::from(100_u64),
                        Currency::Eur,
                    )),
                    display: Some(ProductListingHistoryPriceDisplay {
                        price: Price::new(
                            MonetaryAmount::from(if index < 2 { 200_u64 } else { 400_u64 }),
                            Currency::Usd,
                        ),
                        fx_rate_id: if index < 2 { first_fx } else { later_fx },
                        captured_at,
                    }),
                };
                ProductListingHistoryEntry {
                    product_listing_id,
                    event_id: Default::default(),
                    occurred_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds((index / 2) as i64),
                    kind: if index == 0 {
                        ProductListingHistoryEntryKind::Discovered(Box::new(
                            ProductListingDiscoveryHistory {
                                listing_source_id: ListingSourceId::new(),
                                source_listing_id: SourceListingId::try_from("source-42").unwrap(),
                                title: None,
                                description: None,
                                pricing: ProductListingHistoryPricing {
                                    price: Some(price),
                                    ..Default::default()
                                },
                                availability: None,
                                url: url::Url::parse("https://example.test/listing").unwrap(),
                                image_count: 2,
                                auction: None,
                            },
                        ))
                    } else {
                        ProductListingHistoryEntryKind::Changed(
                            vec![ProductListingHistoryChange::MainPriceChanged {
                                previous: None,
                                current: Some(price),
                            }]
                            .try_into()
                            .unwrap(),
                        )
                    },
                }
            })
            .collect()
    }
    fn assert_output_schema(operation: &OperationDefinition, value: &Value) {
        let schema =
            serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            validator.is_valid(value),
            "{}",
            validator
                .iter_errors(value)
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn should_register_the_exact_public_history_operation_without_paging_arguments() {
        let (registry, _) = setup(Outcome::History(vec![]));
        let operation = operation(&registry);
        assert_eq!(1, registry.page(None).unwrap().0.len());
        assert_eq!(AccessClass::Public, operation.policy().access);
        assert!(operation.policy().required_scopes.is_empty());
        let schema = serde_json::to_value(&operation.tool().input_schema).unwrap();
        assert_eq!(
            vec!["currency", "productListingId"],
            schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(json!(["productListingId"]), schema["required"]);
        assert_eq!(json!(false), schema["additionalProperties"]);
        let annotations = operation.tool().annotations.as_ref().unwrap();
        assert_eq!(Some(true), annotations.read_only_hint);
        assert_eq!(Some(true), annotations.idempotent_hint);
        assert_eq!(Some(false), annotations.destructive_hint);
        assert_eq!(Some(false), annotations.open_world_hint);
    }

    #[tokio::test]
    async fn should_return_every_entry_for_empty_typical_and_large_histories_in_original_order() {
        let product_listing_id = ProductListingId::new();
        for count in [0, 24, 125, 1_001] {
            let entries = entries(product_listing_id, count);
            let expected = serde_json::to_value(
                entries
                    .clone()
                    .into_iter()
                    .map(ProductListingHistoryEntryData::from)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let (registry, service) = setup(Outcome::History(entries.clone()));
            let operation = operation(&registry);
            let response = operation
                .call(input(product_listing_id), context())
                .await
                .unwrap();
            let value = response.structured_content.as_ref().unwrap();
            assert_eq!(json!({"result":expected}), *value);
            assert_eq!(count, value["result"].as_array().unwrap().len());
            assert_eq!(
                *value,
                serde_json::from_str::<Value>(&response.content[0].as_text().unwrap().text)
                    .unwrap()
            );
            assert_output_schema(operation, value);
            assert_eq!(1, service.calls.lock().unwrap().len());
            if count > 1 {
                assert_eq!(
                    value["result"][0]["timestamp"],
                    value["result"][1]["timestamp"]
                );
                assert_ne!(value["result"][0]["eventId"], value["result"][1]["eventId"]);
                assert_eq!(
                    entries.last().unwrap().event_id.to_string(),
                    value["result"][count - 1]["eventId"]
                );
            }
        }
    }

    #[tokio::test]
    async fn should_preserve_each_events_distinct_historical_fx_provenance_and_missing_display() {
        let product_listing_id = ProductListingId::new();
        let mut history = entries(product_listing_id, 4);
        let ProductListingHistoryEntryKind::Changed(changes) = &mut history[3].kind else {
            panic!("changed event")
        };
        *changes = vec![ProductListingHistoryChange::MainPriceChanged {
            previous: Some(ProductListingHistoryPrice::from(
                ProductListingPrice::OnRequest,
            )),
            current: Some(ProductListingHistoryPrice::from(
                ProductListingPrice::Monetary(Price::new(
                    MonetaryAmount::from(100_u64),
                    Currency::Eur,
                )),
            )),
        }]
        .try_into()
        .unwrap();
        let (registry, _) = setup(Outcome::History(history));
        let operation = operation(&registry);
        let response = operation
            .call(input(product_listing_id), context())
            .await
            .unwrap();
        let value = response.structured_content.unwrap();
        assert_output_schema(operation, &value);
        let first = &value["result"][0]["payload"]["pricing"]["price"];
        let later = &value["result"][2]["payload"]["changes"][0]["current"];
        assert_eq!(100, first["amount"]);
        assert_eq!(100, later["amount"]);
        assert_eq!(200, first["display"]["amount"]);
        assert_eq!(400, later["display"]["amount"]);
        assert_ne!(first["display"]["fxRateId"], later["display"]["fxRateId"]);
        assert_ne!(
            first["display"]["capturedAt"],
            later["display"]["capturedAt"]
        );
        let missing = &value["result"][3]["payload"]["changes"][0];
        assert_eq!(json!({"type":"ON_REQUEST"}), missing["previous"]);
        assert_eq!(
            json!({"type":"MONETARY","currency":"EUR","amount":100}),
            missing["current"]
        );
    }

    #[tokio::test]
    async fn should_forward_the_existing_lookup_optional_currency_and_anonymous_context() {
        let product_listing_id = ProductListingId::new();
        for (arguments, currency) in [
            (json!({"productListingId":product_listing_id}), None),
            (
                json!({"productListingId":product_listing_id,"currency":null}),
                None,
            ),
            (input(product_listing_id), Some(Currency::Usd)),
        ] {
            let (registry, service) = setup(Outcome::History(vec![]));
            operation(&registry)
                .call(arguments, context())
                .await
                .unwrap();
            let calls = service.calls.lock().unwrap();
            assert_eq!(1, calls.len());
            assert!(matches!(calls[0].0.principal, Principal::Anonymous));
            assert_eq!(RequestId::new("history-request"), calls[0].0.request_id);
            assert_eq!(
                CorrelationId::new("history-correlation"),
                calls[0].0.correlation_id
            );
            assert_eq!(
                GetProductListingHistoryRequest {
                    lookup: ProductListingHistoryLookup::ById(product_listing_id),
                    currency
                },
                calls[0].1
            );
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_identifiers_currencies_and_paging_before_the_use_case() {
        let id = ProductListingId::new();
        let (registry, service) = setup(Outcome::History(vec![]));
        let operation = operation(&registry);
        let schema = serde_json::to_value(&operation.tool().input_schema).unwrap();
        for arguments in [
            json!({}),
            json!({"productListingId":"invalid"}),
            json!({"productListingId":ListingSourceId::new()}),
            json!({"productListingId":id,"currency":"XXX"}),
            json!({"productListingId":id,"currency":"usd"}),
            json!({"productListingId":id,"currency":5}),
            json!({"productListingId":id,"size":100}),
            json!({"productListingId":id,"after":"cursor"}),
            json!({"productListingId":id,"searchAfter":[]}),
        ] {
            assert!(
                !jsonschema::is_valid(&schema, &arguments),
                "schema {arguments}"
            );
            assert!(matches!(
                operation.call(arguments, context()).await,
                Err(OperationError::InvalidArguments(_))
            ));
        }
        assert!(service.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_distinguish_not_found_from_an_existing_empty_history() {
        let id = ProductListingId::new();
        let (missing, _) = setup(Outcome::NotFound);
        let error = operation(&missing)
            .call(input(id), context())
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::NotFound));
        let protocol = error.into_protocol().unwrap();
        assert_eq!(Some(true), protocol.is_error);
        assert_eq!("NOT_FOUND", protocol.content[0].as_text().unwrap().text);
        let (existing, _) = setup(Outcome::History(vec![]));
        let response = operation(&existing)
            .call(input(id), context())
            .await
            .unwrap();
        assert_eq!(Some(json!({"result":[]})), response.structured_content);
    }

    #[tokio::test]
    async fn should_fail_explicitly_when_the_complete_history_exceeds_the_byte_limit() {
        let id = ProductListingId::new();
        let (registry, service) = setup(Outcome::History(entries(id, 125)));
        let mut context = context();
        context.max_response_bytes = 1024;
        let error = operation(&registry)
            .call(input(id), context)
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::ResultTooLarge));
        let protocol = error.into_protocol().unwrap_err();
        assert_eq!(
            "Result exceeds transport limit; narrow the request",
            protocol.message
        );
        assert_eq!(1, service.calls.lock().unwrap().len());
    }

    #[tokio::test]
    async fn should_bound_pending_history_by_cancellation_and_deadline() {
        let id = ProductListingId::new();
        for expired in [false, true] {
            let (registry, service) = setup(Outcome::Pending);
            let mut context = context();
            if expired {
                context.deadline = tokio::time::Instant::now();
            } else {
                context.cancellation.cancel();
            }
            let result = operation(&registry).call(input(id), context).await;
            assert!(matches!(result, Err(OperationError::Unavailable(_))));
            assert!(service.calls.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn should_preserve_technical_causes_and_expose_only_safe_error_categories() {
        use std::error::Error;
        for error in [
            GetProductListingHistoryError::QueryFailed {
                source: static_error("private database detail"),
            },
            GetProductListingHistoryError::FxSnapshotUnavailable {
                source: static_error("private provider detail"),
            },
            GetProductListingHistoryError::BeginTransactionFailed(static_error(
                "private begin detail",
            )),
            GetProductListingHistoryError::CommitTransactionFailed(static_error(
                "private commit detail",
            )),
            GetProductListingHistoryError::InvalidReadModel {
                source: static_error("private row detail"),
            },
            GetProductListingHistoryError::PriceConversionFailed(
                fxrate_core::FxRateSnapshotError::MissingQuote(Currency::Usd),
            ),
        ] {
            let error = history_error(error);
            assert!(error.source().is_some());
            let response = error.into_protocol();
            match response {
                Ok(result) => assert_eq!(
                    "TEMPORARILY_UNAVAILABLE",
                    result.content[0].as_text().unwrap().text
                ),
                Err(error) => assert_eq!("Operation failed", error.message),
            }
        }
    }

    #[tokio::test]
    async fn should_discover_and_invoke_complete_history_anonymously_at_the_protocol_boundary() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let id = ProductListingId::new();
        let (registry, service) = setup(Outcome::History(entries(id, 125)));
        let app = crate::router(
            crate::TransportConfig::default(),
            registry,
            Arc::new(crate::AnonymousGate),
        )
        .unwrap();
        for (method, mut params) in [
            ("tools/list", json!({})),
            (
                "tools/call",
                json!({"name":"getProductListingHistory","arguments":input(id)}),
            ),
        ] {
            params["_meta"] = json!({
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"history-test","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{}
            });
            let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
            let mut builder = Request::builder()
                .method("POST")
                .uri("/v1")
                .header("host", "localhost:8080")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", method);
            if method == "tools/call" {
                builder = builder.header("mcp-name", "getProductListingHistory");
            }
            let response = app
                .clone()
                .oneshot(builder.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap();
            assert_eq!(StatusCode::OK, response.status());
            let value: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                    .await
                    .unwrap(),
            )
            .unwrap();
            if method == "tools/list" {
                assert_eq!(
                    "getProductListingHistory",
                    value["result"]["tools"][0]["name"]
                );
            } else {
                assert_eq!(
                    125,
                    value["result"]["structuredContent"]["result"]
                        .as_array()
                        .unwrap()
                        .len()
                );
            }
        }
        let calls = service.calls.lock().unwrap();
        assert_eq!(1, calls.len());
        assert!(matches!(calls[0].0.principal, Principal::Anonymous));
    }
}
