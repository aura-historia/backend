use super::*;
use crate::auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal};
use application::operation_context::CredentialCapability;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    routing::delete,
};
use listing_source_core::ListingSourceId;
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome,
        ProductListingIngestionRejectionReason, SubmitPartnerProductListingIngestionHandler,
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
    writer: bool,
    user_id: UserId,
}
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        if token != "token" {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(TransportPrincipal::User {
            user_id: self.user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: if self.writer {
                BTreeSet::from([CredentialCapability::ProductListingsWrite])
            } else {
                BTreeSet::new()
            },
        })
    }
}

#[derive(Clone, Copy, Default)]
enum Mode {
    #[default]
    Accept,
    Mixed,
    Unconfirmed,
    Size,
    Internal,
}
#[derive(Clone, Default)]
struct Publisher {
    calls: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
    mode: Mode,
}
#[async_trait::async_trait]
impl ProductListingIngestionPublisher for Publisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let outcomes = messages
            .iter()
            .map(|message| ProductListingIngestionItemOutcome {
                index: message.metadata.index,
                command_id: message.metadata.command_id.clone(),
                outcome: match self.mode {
                    Mode::Accept => ProductListingIngestionOutcome::Accepted,
                    Mode::Unconfirmed => ProductListingIngestionOutcome::Unconfirmed,
                    Mode::Size => ProductListingIngestionOutcome::Rejected {
                        reason: ProductListingIngestionRejectionReason::Publisher {
                            code: "INVALID_MESSAGE_SIZE".into(),
                        },
                        retryable: false,
                    },
                    Mode::Internal => ProductListingIngestionOutcome::Rejected {
                        reason: ProductListingIngestionRejectionReason::Publisher {
                            code: "INGESTION_INTERNAL_ERROR".into(),
                        },
                        retryable: false,
                    },
                    Mode::Mixed => match message.metadata.index {
                        1 => ProductListingIngestionOutcome::Unconfirmed,
                        2 => ProductListingIngestionOutcome::NotAttempted {
                            reason:
                                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                            retryable: true,
                        },
                        3 => ProductListingIngestionOutcome::Rejected {
                            reason: ProductListingIngestionRejectionReason::Publisher {
                                code: "SQS_RETRYABLE_FAILURE".into(),
                            },
                            retryable: true,
                        },
                        4 => ProductListingIngestionOutcome::Rejected {
                            reason: ProductListingIngestionRejectionReason::Publisher {
                                code: "INVALID_MESSAGE_SIZE".into(),
                            },
                            retryable: false,
                        },
                        _ => ProductListingIngestionOutcome::Accepted,
                    },
                },
            })
            .collect();
        self.calls.lock().unwrap().push(messages);
        Ok(outcomes)
    }
}
fn app(publisher: Publisher, writer: bool) -> Router {
    Router::new()
        .route(
            "/api/v1/listing-sources/{listing_source_id}/product-listings/async",
            delete(delete_products),
        )
        .with_state(AsyncPartnerProductListingsState::new(
            Arc::new(SubmitPartnerProductListingIngestionHandler::new(publisher)),
            Arc::new(Auth {
                writer,
                user_id: UserId::new(),
            }),
        ))
}
fn path(id: ListingSourceId) -> String {
    format!("/api/v1/listing-sources/{id}/product-listings/async")
}
async fn request(
    app: &Router,
    path: &str,
    body: &str,
    key: Option<&str>,
    bearer: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method("DELETE").uri(path);
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    if let Some(token) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
async fn report(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn withdrawal_is_indexed_and_lossless_on_unchanged_retry() {
    let publisher = Publisher::default();
    let calls = publisher.calls.clone();
    let app = app(publisher, true);
    let source = ListingSourceId::new();
    let body = "[{\"sourceListingId\":\"\\u2003SKU  #42/Blue\\u2002\",\"listingSourceId\":\"untrusted\",\"actor\":\"SYSTEM\",\"operation\":\"CREATE\"},null,{\"sourceListingId\":\"SKU  #42/Blue\"},{\"sourceListingId\":\"missing\"},{\"sourceListingId\":\"already-withdrawn\"}]";
    let first = request(&app, &path(source), body, Some("retry-key"), Some("token")).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(first.headers()["idempotency-key"], "retry-key");
    let first = report(first).await;
    assert!(
        first["submissionId"]
            .as_str()
            .unwrap()
            .starts_with("plis1_")
    );
    assert_eq!(first["acceptedCount"], 4);
    assert_eq!(
        first["failures"],
        json!([{"index":1,"error":"BAD_BODY_VALUE","retryable":false}])
    );
    let second =
        report(request(&app, &path(source), body, Some("retry-key"), Some("token")).await).await;
    assert_eq!(first, second);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for messages in calls.iter() {
        assert_eq!(
            messages
                .iter()
                .map(|m| m.metadata.index)
                .collect::<Vec<_>>(),
            [0, 2, 3, 4]
        );
        for (message, id) in messages.iter().zip([
            "SKU  #42/Blue",
            "SKU  #42/Blue",
            "missing",
            "already-withdrawn",
        ]) {
            assert_eq!(message.metadata.input_count, 5);
            assert_eq!(message.metadata.listing_source_id, source);
            let ProductListingIngestionIntent::Withdraw(key) = &message.intent else {
                panic!("expected WITHDRAW")
            };
            assert_eq!(key.listing_source_id, source);
            assert_eq!(key.source_listing_id.as_ref(), id);
            let wire = product_listing_ingestion_sqs::codec::encode(message).unwrap();
            let decoded = product_listing_ingestion_sqs::codec::decode(&wire).unwrap();
            let ProductListingIngestionIntent::Withdraw(decoded_key) =
                decoded.into_intent().unwrap()
            else {
                panic!("expected WITHDRAW")
            };
            assert_eq!(decoded_key, *key);
        }
    }
    for (a, b) in calls[0].iter().zip(&calls[1]) {
        assert_eq!(a.metadata.command_id, b.metadata.command_id);
    }
    assert_ne!(
        calls[0][0].metadata.command_id,
        calls[0][1].metadata.command_id
    );
}

#[tokio::test]
async fn one_hundred_and_empty_are_admitted_after_scope_checks() {
    let publisher = Publisher::default();
    let calls = publisher.calls.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let one = request(
        &app,
        &path,
        r#"[{"sourceListingId":"one"}]"#,
        None,
        Some("token"),
    )
    .await;
    assert_eq!(one.status(), StatusCode::ACCEPTED);
    assert!(one.headers().contains_key("idempotency-key"));
    assert_eq!(report(one).await["acceptedCount"], 1);
    let body = format!("[{}]", vec![r#"{"sourceListingId":"same"}"#; 100].join(","));
    let hundred = request(&app, &path, &body, None, Some("token")).await;
    assert_eq!(hundred.status(), StatusCode::ACCEPTED);
    assert_eq!(report(hundred).await["acceptedCount"], 100);
    let empty = request(&app, &path, "[]", None, Some("token")).await;
    assert_eq!(empty.status(), StatusCode::ACCEPTED);
    assert!(empty.headers().contains_key("idempotency-key"));
    assert_eq!(report(empty).await["failures"], json!([]));
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        [1, 100]
    );
}

#[tokio::test]
async fn malformed_members_fail_independently_without_trusting_unknown_fields() {
    let publisher = Publisher::default();
    let calls = publisher.calls.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let invalid = [
        "null",
        "123",
        r#"{"sourceListingId":12}"#,
        "{}",
        r#"{"sourceListingId":"  "}"#,
        r#"{"sourceListingId":"bad\u0000id"}"#,
        r#"{"sourceListingId":"duplicate","sourceListingId":"duplicate"}"#,
        &format!(r#"{{"sourceListingId":"{}"}}"#, "é".repeat(257)),
    ];
    let body = format!(
        "[{{\"sourceListingId\":\"good\"}},{},{{\"sourceListingId\":\"last\",\"scope\":\"forged\"}},false]",
        invalid.join(",")
    );
    let result = request(&app, &path, &body, Some("mixed"), Some("token")).await;
    assert_eq!(result.status(), StatusCode::ACCEPTED);
    let result = report(result).await;
    assert_eq!(result["acceptedCount"], 2);
    assert_eq!(result["failures"].as_array().unwrap().len(), 9);
    for (i, failure) in result["failures"].as_array().unwrap().iter().enumerate() {
        assert_eq!(failure["index"], if i == 8 { 10 } else { i + 1 });
        assert_eq!(failure["error"], "BAD_BODY_VALUE");
        assert_eq!(failure["retryable"], false);
        assert!(failure.get("sourceListingId").is_none());
    }
    assert_eq!(
        calls.lock().unwrap()[0]
            .iter()
            .map(|m| m.metadata.index)
            .collect::<Vec<_>>(),
        [0, 9]
    );
    let all_invalid = request(
        &app,
        &path,
        &format!("[{}]", invalid.join(",")),
        None,
        Some("token"),
    )
    .await;
    assert_eq!(all_invalid.status(), StatusCode::BAD_REQUEST);
    let body = report(all_invalid).await;
    assert_eq!(body["acceptedCount"], 0);
    assert_eq!(body["failures"].as_array().unwrap().len(), invalid.len());
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn request_wide_errors_never_publish() {
    let publisher = Publisher::default();
    let calls = publisher.calls.clone();
    let app = app(publisher.clone(), true);
    let path = path(ListingSourceId::new());
    for body in [
        "",
        "{}",
        r#"[{"sourceListingId":"first"},"#,
        r#"[{"sourceListingId":"first"},]"#,
        &format!("[{}]", vec![r#"{"sourceListingId":"same"}"#; 101].join(",")),
    ] {
        assert_eq!(
            request(&app, &path, body, None, Some("token"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            &app,
            "/api/v1/listing-sources/bad/product-listings/async",
            "[]",
            None,
            Some("token")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, &path, "[]", Some("bad key"), Some("token"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&app, &path, "[]", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &path, "[]", None, Some("invalid"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &self::app(publisher, false),
            &path,
            "[]",
            None,
            Some("token")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let limited =
        crate::transport::with_transport_middleware(app, crate::transport::NATIVE_REQUEST_TIMEOUT);
    let oversized = request(&limited, &path, &"x".repeat(1_048_577), None, Some("token")).await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        oversized.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn partial_and_zero_confirmed_publisher_outcomes_are_truthful() {
    let publisher = Publisher {
        mode: Mode::Mixed,
        ..Default::default()
    };
    let calls = publisher.calls.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let body = r#"[null,{"sourceListingId":"same"},{"sourceListingId":"same"},{"sourceListingId":"failed"},{"sourceListingId":"oversize"},{"sourceListingId":"other"}]"#;
    let first = request(&app, &path, body, Some("retry"), Some("token")).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = report(first).await;
    assert_eq!(first["acceptedCount"], 1);
    assert_eq!(
        first["failures"],
        json!([
            {"index":0,"error":"BAD_BODY_VALUE","retryable":false},
            {"index":1,"sourceListingId":"same","error":"ENQUEUE_UNCONFIRMED","retryable":true},
            {"index":2,"sourceListingId":"same","error":"ENQUEUE_BLOCKED","retryable":true},
            {"index":3,"sourceListingId":"failed","error":"ENQUEUE_FAILED","retryable":true},
            {"index":4,"sourceListingId":"oversize","error":"PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE","retryable":false}
        ])
    );
    let second = report(request(&app, &path, body, Some("retry"), Some("token")).await).await;
    assert_eq!(first, second);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for call in calls.iter() {
        assert_eq!(
            call.iter().map(|m| m.metadata.index).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
    }
    for (a, b) in calls[0].iter().zip(&calls[1]) {
        assert_eq!(a.metadata.command_id, b.metadata.command_id);
    }
    drop(calls);
    for (mode, expected_status, error) in [
        (
            Mode::Unconfirmed,
            StatusCode::SERVICE_UNAVAILABLE,
            "ENQUEUE_UNCONFIRMED",
        ),
        (
            Mode::Size,
            StatusCode::PAYLOAD_TOO_LARGE,
            "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
        ),
        (
            Mode::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
            "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
        ),
    ] {
        let app = self::app(
            Publisher {
                mode,
                ..Default::default()
            },
            true,
        );
        let result = request(
            &app,
            &path,
            r#"[{"sourceListingId":"one"}]"#,
            None,
            Some("token"),
        )
        .await;
        assert_eq!(result.status(), expected_status);
        let result = report(result).await;
        assert_eq!(result["acceptedCount"], 0);
        assert_eq!(result["failures"][0]["error"], error);
    }
}
