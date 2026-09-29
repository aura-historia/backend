use super::{
    async_batch,
    types::{CreateProductListingData, parse_listing_source_id},
};
use crate::state::AsyncPartnerProductListingsState;

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};

use product_listing_service::use_cases::{
    ProductListingIngestionIntent, ProductListingIngestionSubmission,
};

pub async fn create_products(
    State(state): State<AsyncPartnerProductListingsState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: String,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let key = match async_batch::idempotency_key(&headers) {
        Ok(key) => key,
        Err(error) => return error.into_response(),
    };
    let context =
        match async_batch::authorized_context(state.authenticator.as_ref(), &headers).await {
            Ok(context) => context,
            Err(response) => return response,
        };
    let raw_items = match async_batch::parse_envelope(&body) {
        Ok(items) => items,
        Err(error) => return error.into_response(),
    };
    let count = raw_items.len();
    let (items, source_ids, failures) = async_batch::parse_items::<CreateProductListingData>(
        raw_items,
        |product| &product.source_listing_id,
        |product| {
            product
                .into_command(listing_source_id)
                .map(ProductListingIngestionIntent::Create)
        },
    );
    let result = state
        .submit
        .execute(
            &context,
            ProductListingIngestionSubmission {
                listing_source_id,
                original_input_count: count,
                idempotency_key: key,
                items,
            },
        )
        .await;
    match result {
        Ok(result) => async_batch::report(result, failures, &source_ids),
        Err(error) => async_batch::submission_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use application::operation_context::CredentialCapability;
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
        routing::post,
    };
    use listing_source_core::ListingSourceId;
    use product_listing_service::{
        ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
        use_cases::{
            ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
            ProductListingIngestionOutcome, ProductListingIngestionRejectionReason,
            SubmitPartnerProductListingIngestionHandler,
        },
    };
    use serde_json::{Value, json};
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    struct Auth {
        capability: bool,
        user_id: UserId,
    }
    #[async_trait::async_trait]
    impl TokenAuthenticator for Auth {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: self.user_id,
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: if self.capability {
                    BTreeSet::from([CredentialCapability::ProductListingsWrite])
                } else {
                    BTreeSet::new()
                },
            })
        }
    }

    #[derive(Clone, Default)]
    struct Publisher {
        sent: Arc<Mutex<Vec<Vec<usize>>>>,
        uncertain: bool,
        reject_size: bool,
        reject_internal: bool,
    }
    #[async_trait::async_trait]
    impl ProductListingIngestionPublisher for Publisher {
        async fn publish(
            &self,
            messages: Vec<ProductListingIngestionMessage>,
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
            self.sent.lock().unwrap().push(
                messages
                    .iter()
                    .map(|message| message.metadata.index)
                    .collect(),
            );
            Ok(messages
                .into_iter()
                .map(|message| ProductListingIngestionItemOutcome {
                    index: message.metadata.index,
                    command_id: message.metadata.command_id,
                    outcome: if self.uncertain {
                        ProductListingIngestionOutcome::Unconfirmed
                    } else if self.reject_size && message.metadata.index == 0 {
                        ProductListingIngestionOutcome::Rejected {
                            reason: ProductListingIngestionRejectionReason::Publisher {
                                code: "INVALID_MESSAGE_SIZE".into(),
                            },
                            retryable: false,
                        }
                    } else if self.reject_internal && message.metadata.index == 0 {
                        ProductListingIngestionOutcome::Rejected {
                            reason: ProductListingIngestionRejectionReason::Publisher {
                                code: "INGESTION_INTERNAL_ERROR".into(),
                            },
                            retryable: false,
                        }
                    } else {
                        ProductListingIngestionOutcome::Accepted
                    },
                })
                .collect())
        }
    }
    fn app(publisher: Publisher, capability: bool) -> Router {
        let state = AsyncPartnerProductListingsState::new(
            Arc::new(SubmitPartnerProductListingIngestionHandler::new(publisher)),
            Arc::new(Auth {
                capability,
                user_id: UserId::new(),
            }),
        );
        Router::new()
            .route(
                "/api/v1/listing-sources/{listing_source_id}/product-listings/async",
                post(create_products),
            )
            .with_state(state)
    }
    fn product(id: &str) -> String {
        format!(
            r#"{{"sourceListingId":"{id}","title":{{"text":"Cabinet","language":"en"}},"description":{{"text":"Old cabinet","language":"en"}},"url":"https://example.test/product","images":[]}}"#
        )
    }
    async fn request(
        app: &Router,
        id: ListingSourceId,
        body: &str,
        key: Option<&str>,
        authenticated: bool,
    ) -> Response {
        let mut request = Request::builder().method("POST").uri(format!(
            "/api/v1/listing-sources/{id}/product-listings/async"
        ));
        if authenticated {
            request = request.header(header::AUTHORIZATION, "Bearer token");
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        app.clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }
    async fn json_body(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn admits_valid_siblings_without_compacting_original_indices() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let body = format!(
            "[{},null,{},{}]",
            product("one"),
            product("two"),
            product("three").replace("\"images\":[]", "\"images\":[],\"auction\":null")
        );
        let response = request(&app, id, &body, Some("client-key"), true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()["idempotency-key"], "client-key");
        let body = json_body(response).await;
        assert!(body["submissionId"].as_str().unwrap().starts_with("plis1_"));
        assert_eq!(body["acceptedCount"], 2);
        assert_eq!(
            body["failures"],
            json!([
                {"index":1,"error":"BAD_BODY_VALUE","retryable":false},
                {"index":3,"sourceListingId":"three","error":"BAD_BODY_VALUE","retryable":false}
            ])
        );
        assert_eq!(*sent.lock().unwrap(), vec![vec![0, 2]]);
    }

    #[tokio::test]
    async fn empty_and_all_invalid_have_key_and_no_sends_after_authorization() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let empty = request(&app, id, "[]", None, true).await;
        assert_eq!(empty.status(), StatusCode::ACCEPTED);
        assert!(empty.headers().contains_key("idempotency-key"));
        assert_eq!(json_body(empty).await["acceptedCount"], 0);
        let invalid = request(&app, id, "[null,1]", Some("same"), true).await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(invalid.headers()["idempotency-key"], "same");
        assert_eq!(
            json_body(invalid).await["failures"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn uncertainty_and_parsing_failures_keep_report_shape_and_status() {
        let publisher = Publisher {
            uncertain: true,
            ..Publisher::default()
        };
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let response = request(
            &app,
            id,
            &format!("[null,{}]", product("valid")),
            Some("retry-key"),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json_body(response).await;
        assert_eq!(body["acceptedCount"], 0);
        assert_eq!(
            body["failures"],
            json!([
                {"index":0,"error":"BAD_BODY_VALUE","retryable":false},
                {"index":1,"sourceListingId":"valid","error":"ENQUEUE_UNCONFIRMED","retryable":true}
            ])
        );
        assert_eq!(*sent.lock().unwrap(), vec![vec![1]]);
    }

    #[tokio::test]
    async fn rejects_whole_request_before_sends() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        for body in [
            "",
            "{}",
            "[{},]",
            "[null,",
            &format!("[{}]", vec!["null"; 101].join(",")),
        ] {
            assert_eq!(
                request(&app, id, body, None, true).await.status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            request(&app, id, "[]", None, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert!(sent.lock().unwrap().is_empty());
        let restricted = self::app(Publisher::default(), false);
        assert_eq!(
            request(&restricted, id, "[]", None, true).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&app, id, "[]", Some("invalid key"), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn preserves_strict_item_semantics_and_admits_one_hundred() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let bad_duplicate = product("dup").replace("\"images\":[]", "\"images\":[],\"images\":[]");
        let bad_unknown =
            product("unknown").replace("\"images\":[]", "\"images\":[],\"extraneous\":true");
        let body = format!("[{},{},{}]", product("valid"), bad_duplicate, bad_unknown);
        let response = request(&app, id, &body, None, true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(
            json_body(response).await["failures"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let body = format!(
            "[{}]",
            (0..100)
                .map(|i| product(&format!("sku-{i}")))
                .collect::<Vec<_>>()
                .join(",")
        );
        let response = request(&app, id, &body, Some("hundred"), true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let result = json_body(response).await;
        assert_eq!(result["acceptedCount"], 100);
        assert_eq!(result["failures"], json!([]));
        assert_eq!(sent.lock().unwrap()[1].len(), 100);
    }

    #[tokio::test]
    async fn invalid_create_values_are_independent_item_failures() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let invalid = [
            product(" "),
            product("bad-url").replace("https://example.test/product", "not-a-url"),
            product("bad-availability").replace(
                "\"images\":[]",
                "\"images\":[],\"availability\":\"NOT_A_STATUS\"",
            ),
            product("bad-price").replace(
                "\"images\":[]",
                "\"images\":[],\"price\":{\"type\":\"INVALID\"}",
            ),
            product("bad-auction").replace(
                "\"images\":[]",
                "\"images\":[],\"auction\":{\"auctionId\":\"invalid\"}",
            ),
            product("missing").replace("\"sourceListingId\":\"missing\",", ""),
        ];
        let body = format!("[{},{}]", invalid.join(","), product("accepted"));
        let response = request(&app, id, &body, Some("values"), true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let report = json_body(response).await;
        assert_eq!(report["acceptedCount"], 1);
        let failures = report["failures"].as_array().unwrap();
        assert_eq!(failures.len(), invalid.len());
        assert_eq!(
            failures
                .iter()
                .map(|f| f["index"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            (0..invalid.len() as u64).collect::<Vec<_>>()
        );
        assert!(failures.iter().all(|f| f["retryable"] == false));
        assert!(failures[5].get("sourceListingId").is_none());
        assert_eq!(*sent.lock().unwrap(), vec![vec![6]]);
    }

    #[tokio::test]
    async fn whole_body_limit_rejects_before_handler_sends() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = crate::transport::with_transport_middleware(
            app(publisher, true),
            crate::transport::NATIVE_REQUEST_TIMEOUT,
        );
        let response = request(
            &app,
            ListingSourceId::new(),
            &"x".repeat(1_048_577),
            Some("key"),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(json_body(response).await["error"], "BAD_BODY_VALUE");
        assert!(sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn evaluated_size_and_internal_failures_preserve_report_and_key() {
        let id = ListingSourceId::new();
        let body = format!("[{}]", product("large"));
        let size = app(
            Publisher {
                reject_size: true,
                ..Publisher::default()
            },
            true,
        );
        let response = request(&size, id, &body, Some("size"), true).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.headers()["idempotency-key"], "size");
        assert_eq!(
            json_body(response).await["failures"][0]["error"],
            "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE"
        );
        let mixed = request(
            &size,
            id,
            &format!("[{},{}]", product("large"), product("good")),
            Some("mixed"),
            true,
        )
        .await;
        assert_eq!(mixed.status(), StatusCode::ACCEPTED);
        let body = json_body(mixed).await;
        assert_eq!(body["acceptedCount"], 1);
        assert_eq!(body["failures"].as_array().unwrap().len(), 1);
        let internal = app(
            Publisher {
                reject_internal: true,
                ..Publisher::default()
            },
            true,
        );
        let response = request(
            &internal,
            id,
            &format!("[{}]", product("broken")),
            Some("internal"),
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            json_body(response).await["failures"][0]["error"],
            "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR"
        );
    }

    #[tokio::test]
    async fn unchanged_sparse_batch_retry_keeps_submission_id_and_positions() {
        let publisher = Publisher {
            uncertain: true,
            ..Publisher::default()
        };
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let body = format!("[null,{},{}]", product("first"), product("second"));
        let first = request(&app, id, &body, Some("retry-same"), true).await;
        let second = request(&app, id, &body, Some("retry-same"), true).await;
        assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
        let first = json_body(first).await;
        let second = json_body(second).await;
        assert_eq!(first["submissionId"], second["submissionId"]);
        assert_eq!(first["failures"], second["failures"]);
        assert_eq!(*sent.lock().unwrap(), vec![vec![1, 2], vec![1, 2]]);
    }
}
