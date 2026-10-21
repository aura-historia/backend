use super::*;
use crate::{MAX_RESPONSE_BYTES, Registry};
use application::operation_context::{CorrelationId, RequestId};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

mod fixtures;
use fixtures::{Fixture, Outcome, Request, location_id, party_id, source_id};

fn context() -> InvocationContext {
    InvocationContext {
        operation: OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("discovery-test"),
            correlation_id: CorrelationId::new("discovery-correlation"),
        },
        deadline: Instant::now() + Duration::from_secs(5),
        cancellation: CancellationToken::new(),
        max_response_bytes: MAX_RESPONSE_BYTES,
    }
}

async fn call(registry: &Registry, name: &str, arguments: Value) -> CallToolResult {
    let operation = registry.get(name).expect("registered family operation");
    assert!(jsonschema::is_valid(
        &serde_json::to_value(&operation.tool().input_schema).unwrap(),
        &arguments
    ));
    let result = operation.call(arguments, context()).await.unwrap();
    let value = result.structured_content.as_ref().unwrap();
    assert!(jsonschema::is_valid(
        &serde_json::to_value(operation.tool().output_schema.as_ref().unwrap()).unwrap(),
        value
    ));
    result
}

async fn invalid(registry: &Registry, name: &str, arguments: Value) {
    let error = registry
        .get(name)
        .unwrap()
        .call(arguments, context())
        .await
        .unwrap_err();
    assert!(
        matches!(&error, OperationError::InvalidArguments(_)),
        "{error:?}"
    );
    assert_eq!(-32602, error.into_protocol().unwrap_err().code.0);
}

#[test]
fn should_register_exactly_four_stable_public_read_operations() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let (tools, next) = registry.page(None).unwrap();
    assert!(next.is_none());
    assert_eq!(
        vec![
            "discoverPartyLocations",
            "getPublicListingSourceBySlug",
            "getPublicOverview",
            "searchPublicListingSources"
        ],
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>()
    );
    for tool in tools {
        let policy = registry.get(&tool.name).unwrap().policy();
        assert_eq!(AccessClass::Public, policy.access);
        assert!(policy.required_scopes.is_empty());
        assert!(policy.read_only && policy.idempotent);
        assert!(!policy.destructive && !policy.open_world);
        assert_eq!(
            tool.name.as_ref(),
            tool.meta.as_ref().unwrap().0["aura.historia/operationId"]
        );
    }
}

#[tokio::test]
async fn should_preserve_browse_insufficient_text_and_search_with_query_bound_continuation() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let browse = call(&registry, "searchPublicListingSources", json!({"size":1}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(1, browse["size"]);
    let cursor = browse["searchAfter"].as_str().unwrap();
    let next = call(
        &registry,
        "searchPublicListingSources",
        json!({"size":1,"searchAfter":cursor}),
    )
    .await
    .structured_content
    .unwrap();
    assert!(next.get("searchAfter").is_none());
    invalid(
        &registry,
        "searchPublicListingSources",
        json!({"size":2,"searchAfter":cursor}),
    )
    .await;
    invalid(
        &registry,
        "searchPublicListingSources",
        json!({"query":"mu","size":1,"searchAfter":cursor}),
    )
    .await;
    invalid(
        &registry,
        "searchPublicListingSources",
        json!({"query":"a","size":1,"searchAfter":cursor}),
    )
    .await;
    let insufficient = call(
        &registry,
        "searchPublicListingSources",
        json!({"query":"a"}),
    )
    .await
    .structured_content
    .unwrap();
    assert_eq!(json!({"items":[],"size":0}), insufficient);
    let search = call(
        &registry,
        "searchPublicListingSources",
        json!({"query":"museum","size":50}),
    )
    .await
    .structured_content
    .unwrap();
    let cursor = search["searchAfter"].as_str().unwrap();
    call(
        &registry,
        "searchPublicListingSources",
        json!({"query":"  museum  ","size":50,"searchAfter":cursor}),
    )
    .await;
    invalid(
        &registry,
        "searchPublicListingSources",
        json!({"query":"other","size":50,"searchAfter":cursor}),
    )
    .await;
    let requests = fixture.requests.lock().unwrap();
    let Request::Search(context, request) = &requests[0] else {
        panic!("source search request")
    };
    assert_eq!(Principal::Anonymous, context.principal);
    assert_eq!("discovery-test", context.request_id.as_str());
    assert!(request.query().is_browse());
    assert_eq!(1, request.page_size());
    assert_eq!(
        5,
        requests.len(),
        "cursor mismatches must not call the use case"
    );
}

#[tokio::test]
async fn should_reject_invalid_source_inputs_before_starting_service_work() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    for input in [
        json!({"size":0}),
        json!({"size":51}),
        json!({"size":"21"}),
        json!({"query":"x".repeat(257)}),
        json!({"query":"ab\u{0000}cd"}),
        json!({"searchAfter":"not-base64"}),
        json!({"searchAfter":"x".repeat(4097)}),
        json!({"privateGrant":true}),
    ] {
        invalid(&registry, "searchPublicListingSources", input).await;
    }
    for input in [
        json!({"listingSourceSlugId":"Museum"}),
        json!({"listingSourceSlugId":""}),
        json!({"listingSourceSlugId":"museum","contact":true}),
    ] {
        invalid(&registry, "getPublicListingSourceBySlug", input).await;
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn should_preserve_all_public_source_fields_without_management_configuration() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let detail = call(
        &registry,
        "getPublicListingSourceBySlug",
        json!({"listingSourceSlugId":"museum-source"}),
    )
    .await
    .structured_content
    .unwrap();
    assert_eq!(
        json!({"listingSourceId":source_id(),"listingSourceSlugId":"museum-source","name":"Museum Source","operator":{"name":"Museum Operator"},"url":"https://museum.example.test/","image":"https://museum.example.test/card.jpg"}),
        detail
    );
    let search = call(&registry, "searchPublicListingSources", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(detail, search["items"][0]);
    let requests = fixture.requests.lock().unwrap();
    let Request::Detail(context, request) = &requests[0] else {
        panic!("slug detail request")
    };
    assert_eq!(Principal::Anonymous, context.principal);
    assert_eq!("museum-source", request.slug_id.as_ref());
}

fn full_location_arguments() -> Value {
    json!({"geography":{"countries":["DE"],"subdivisions":["DE-BY"],"origin":{"latitude":48.137,"longitude":11.575},"radius":{"amount":25.0,"unit":"KILOMETERS"},"mode":"APPROXIMATE","distanceOrder":"DESC"},"size":100,"searchAfter":{"locationId":location_id(),"distanceKm":42.25}})
}

#[tokio::test]
async fn should_pass_complete_geography_paging_and_public_location_fields() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let result = call(
        &registry,
        "discoverPartyLocations",
        full_location_arguments(),
    )
    .await
    .structured_content
    .unwrap();
    assert_eq!(
        json!({"items":[{"partyId":party_id(),"partyName":"Museum Operator","locationId":location_id(),"label":"Gallery","basis":"ACCEPTED_DERIVED","country":"DE","subdivision":"DE-BY","point":{"latitude":48.137,"longitude":11.575},"precision":"LOCALITY","distanceKm":23.5}],"size":1,"searchAfter":{"locationId":location_id(),"distanceKm":23.5}}),
        result
    );
    let requests = fixture.requests.lock().unwrap();
    let Request::Location(request) = &requests[0] else {
        panic!("location discovery request")
    };
    assert_eq!(100, request.cursor.size);
    assert_eq!(
        Some(42.25),
        request.cursor.search_after.unwrap().distance_km
    );
    assert_eq!(
        Some(location_id()),
        request.cursor.search_after.map(|cursor| cursor.location_id)
    );
    assert!(
        request
            .geography
            .countries()
            .contains(&geo::CountryCode::DEU)
    );
    assert!(
        request
            .geography
            .subdivisions()
            .contains(&geo::SubdivisionCode::new("DE-BY").unwrap())
    );
    assert_eq!(
        Some(geo::GeoPoint::new(48.137, 11.575).unwrap()),
        request.geography.origin()
    );
    assert_eq!(
        Some(25.0),
        request
            .geography
            .radius()
            .map(|radius| radius.to_kilometers())
    );
    assert_eq!(
        geo::core::discovery::ProximityMode::Approximate,
        request.geography.mode()
    );
    assert_eq!(
        Some(geo::core::discovery::DistanceOrder::Desc),
        request.geography.distance_order()
    );
}

#[tokio::test]
async fn should_retain_country_only_defaults_and_omit_absent_distance() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let result = call(
        &registry,
        "discoverPartyLocations",
        json!({"geography":{"countries":["DE"]}}),
    )
    .await
    .structured_content
    .unwrap();
    assert!(result["items"][0].get("distanceKm").is_none());
    assert_eq!(json!({"locationId":location_id()}), result["searchAfter"]);
    let requests = fixture.requests.lock().unwrap();
    let Request::Location(request) = &requests[0] else {
        panic!("location request")
    };
    assert_eq!(21, request.cursor.size);
    assert!(request.cursor.search_after.is_none());
}

#[tokio::test]
async fn should_reject_invalid_geography_and_service_cursor_bounds() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    for geography in [
        json!({}),
        json!({"countries":["XX"]}),
        json!({"countries":["DE"],"subdivisions":["FR-IDF"]}),
        json!({"radius":{"amount":25,"unit":"KILOMETERS"}}),
        json!({"origin":{"latitude":91,"longitude":0},"distanceOrder":"ASC"}),
        json!({"origin":{"address":"Munich"},"distanceOrder":"ASC"}),
        json!({"countries":["DE"],"disclosure":"PUBLIC"}),
    ] {
        invalid(
            &registry,
            "discoverPartyLocations",
            json!({"geography":geography}),
        )
        .await;
    }
    for input in [
        json!({"geography":{"countries":["DE"]},"size":0}),
        json!({"geography":{"countries":["DE"]},"size":101}),
        json!({"geography":{"countries":["DE"]},"searchAfter":{"locationId":location_id(),"distanceKm":1}}),
        json!({"geography":{"countries":["DE"]},"searchAfter":{"locationId":source_id()}}),
    ] {
        invalid(&registry, "discoverPartyLocations", input).await;
    }
    let mut input = full_location_arguments();
    input["searchAfter"]["distanceKm"] = json!(-1);
    invalid(&registry, "discoverPartyLocations", input).await;
    let mut input = full_location_arguments();
    input["searchAfter"]
        .as_object_mut()
        .unwrap()
        .remove("distanceKm");
    invalid(&registry, "discoverPartyLocations", input).await;
}

#[tokio::test]
async fn should_return_every_public_overview_aggregate_anonymously() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let result = call(&registry, "getPublicOverview", json!({}))
        .await
        .structured_content
        .unwrap();
    assert_eq!(
        json!({"asOf":"2026-10-19T12:34:56.789Z","sourcesIndexed":12,"liveObjects":3456,"countries":7,"languages":5,"newObjectsLast7Days":89}),
        result
    );
    invalid(&registry, "getPublicOverview", json!({"language":"de"})).await;
    let requests = fixture.requests.lock().unwrap();
    let Request::Overview(context) = &requests[0] else {
        panic!("overview request")
    };
    assert_eq!(Principal::Anonymous, context.principal);
    assert_eq!("discovery-correlation", context.correlation_id.as_str());
}

#[tokio::test]
async fn should_share_non_queuing_source_budget_and_release_it_after_timeout() {
    let fixture = Arc::new(Fixture {
        outcome: Outcome::Pending,
        ..Default::default()
    });
    let budget = PublicListingSourceReadBudget::new(1, Duration::from_millis(5));
    let registry = fixture.registry(budget.clone());
    let permit = budget.try_acquire().unwrap();
    for (name, arguments) in [
        ("searchPublicListingSources", json!({})),
        (
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"museum-source"}),
        ),
    ] {
        let error = registry
            .get(name)
            .unwrap()
            .call(arguments, context())
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Unavailable(_)));
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
    drop(permit);
    for (name, arguments) in [
        ("searchPublicListingSources", json!({})),
        (
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"museum-source"}),
        ),
    ] {
        let error = registry
            .get(name)
            .unwrap()
            .call(arguments, context())
            .await
            .unwrap_err();
        let result = error.into_protocol().unwrap();
        assert_eq!(Some(true), result.is_error);
        assert_eq!(
            "TEMPORARILY_UNAVAILABLE",
            result.content[0].as_text().unwrap().text
        );
        assert!(budget.try_acquire().is_some());
    }
}

#[tokio::test]
async fn should_observe_invocation_deadline_and_cancellation_for_every_operation() {
    let fixture = Arc::new(Fixture {
        outcome: Outcome::Pending,
        ..Default::default()
    });
    let budget = PublicListingSourceReadBudget::new(1, Duration::from_secs(5));
    let registry = fixture.registry(budget.clone());
    for (name, arguments) in operation_arguments() {
        let mut expired = context();
        expired.deadline = Instant::now();
        let error = registry
            .get(name)
            .unwrap()
            .call(arguments.clone(), expired)
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Unavailable(_)));
        let cancelled = context();
        cancelled.cancellation.cancel();
        let error = registry
            .get(name)
            .unwrap()
            .call(arguments, cancelled)
            .await
            .unwrap_err();
        assert!(matches!(error, OperationError::Unavailable(_)));
        assert!(budget.try_acquire().is_some());
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}

fn operation_arguments() -> [(&'static str, Value); 4] {
    [
        ("searchPublicListingSources", json!({})),
        (
            "getPublicListingSourceBySlug",
            json!({"listingSourceSlugId":"museum-source"}),
        ),
        (
            "discoverPartyLocations",
            json!({"geography":{"countries":["DE"]}}),
        ),
        ("getPublicOverview", json!({})),
    ]
}

#[tokio::test]
async fn should_keep_service_failures_distinct_and_private() {
    for outcome in [Outcome::Unavailable, Outcome::Internal] {
        let fixture = Arc::new(Fixture {
            outcome,
            ..Default::default()
        });
        let registry = fixture.registry(PublicListingSourceReadBudget::default());
        for (name, arguments) in operation_arguments() {
            let error = registry
                .get(name)
                .unwrap()
                .call(arguments, context())
                .await
                .unwrap_err();
            assert!(std::error::Error::source(&error).is_some());
            let response = match error.into_protocol() {
                Ok(result) => serde_json::to_value(result).unwrap(),
                Err(error) => serde_json::to_value(error).unwrap(),
            };
            assert!(!response.to_string().contains("private provider detail"));
            match outcome {
                Outcome::Unavailable => assert_eq!(true, response["isError"]),
                Outcome::Internal => assert_eq!(-32603, response["code"]),
                _ => unreachable!(),
            }
        }
    }
    let fixture = Arc::new(Fixture {
        outcome: Outcome::Missing,
        ..Default::default()
    });
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let missing = registry
        .get("getPublicListingSourceBySlug")
        .unwrap()
        .call(json!({"listingSourceSlugId":"missing"}), context())
        .await
        .unwrap_err();
    assert!(matches!(missing, OperationError::NotFound));
    let missing = registry
        .get("getPublicOverview")
        .unwrap()
        .call(json!({}), context())
        .await
        .unwrap_err();
    assert!(matches!(missing, OperationError::Unavailable(_)));
}

#[tokio::test]
async fn should_serve_family_manifests_and_shared_results_through_streamable_http() {
    use axum::{
        body::{Body, to_bytes},
        http::Request as HttpRequest,
    };
    use tower::ServiceExt;
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    let app = crate::router(
        crate::TransportConfig::default(),
        registry.clone(),
        Arc::new(crate::AnonymousGate),
    )
    .unwrap();
    for (index, (name, arguments)) in operation_arguments().into_iter().enumerate() {
        let params = json!({"name":name,"arguments":arguments,"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"public-discovery-test","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}});
        let response = app
            .clone()
            .oneshot(
                HttpRequest::post("/v1")
                    .header("host", "localhost:8080")
                    .header("content-type", "application/json")
                    .header("accept", "application/json,text/event-stream")
                    .header("mcp-method", "tools/call")
                    .header("mcp-name", name)
                    .header("mcp-protocol-version", "2026-07-28")
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":index,"method":"tools/call","params":params})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(200, response.status());
        let response: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                .await
                .unwrap(),
        )
        .unwrap();
        let result = &response["result"]["structuredContent"];
        assert!(result.is_object(), "{response}");
        assert!(
            result.get("result").is_none(),
            "shared object shape remains direct"
        );
        let schema = serde_json::to_value(
            registry
                .get(name)
                .unwrap()
                .tool()
                .output_schema
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert!(jsonschema::is_valid(&schema, result));
        let text: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(*result, text);
    }
}

#[tokio::test]
async fn should_preserve_source_context_and_keep_overview_anonymous() {
    let fixture = Arc::new(Fixture::default());
    let registry = fixture.registry(PublicListingSourceReadBudget::default());
    for (name, arguments) in operation_arguments()
        .into_iter()
        .filter(|(name, _)| *name != "discoverPartyLocations")
    {
        let mut invocation = context();
        invocation.operation.principal = Principal::System;
        registry
            .get(name)
            .unwrap()
            .call(arguments, invocation)
            .await
            .unwrap();
    }
    for request in fixture.requests.lock().unwrap().iter() {
        match request {
            Request::Search(context, _) | Request::Detail(context, _) => {
                assert_eq!(Principal::System, context.principal);
            }
            Request::Overview(context) => assert_eq!(Principal::Anonymous, context.principal),
            Request::Location(_) => unreachable!(),
        }
    }
}
