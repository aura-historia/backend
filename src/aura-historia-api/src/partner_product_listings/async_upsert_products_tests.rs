use super::*;
use crate::auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal};
use application::{operation_context::CredentialCapability, patch_field::PatchField};
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
    Size,
    Internal,
}
#[derive(Clone, Default)]
struct Publisher {
    sent: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
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
                    Mode::Size if message.metadata.index == 0 => {
                        ProductListingIngestionOutcome::Rejected {
                            reason: ProductListingIngestionRejectionReason::Publisher {
                                code: "INVALID_MESSAGE_SIZE".into(),
                            },
                            retryable: false,
                        }
                    }
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
                                code: "INVALID_MESSAGE_SIZE".into(),
                            },
                            retryable: false,
                        },
                        _ => ProductListingIngestionOutcome::Accepted,
                    },
                    _ => ProductListingIngestionOutcome::Accepted,
                },
            })
            .collect();
        self.sent.lock().unwrap().push(messages);
        Ok(outcomes)
    }
}
fn app(publisher: Publisher, writer: bool) -> Router {
    Router::new()
        .route(
            "/api/v1/listing-sources/{listing_source_id}/product-listings/async",
            post(crate::partner_product_listings::async_create_products::create_products)
                .put(upsert_products),
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
    let mut builder = Request::builder().method("PUT").uri(path);
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
async fn upsert_admission_preserves_original_indices_and_stable_command_identity() {
    let publisher = Publisher::default();
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let id = ListingSourceId::new();
    let body = r#"[{"sourceListingId":"  SKU-1  "},null,{"sourceListingId":"SKU-1","title":{"text":"One","language":"en"},"description":{"text":"New","language":"en"}}, {"sourceListingId":"bad","images":null}]"#;
    let first = request(&app, &path(id), body, Some("same-key"), Some("token")).await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(first.headers()["idempotency-key"], "same-key");
    let first = report(first).await;
    assert!(
        first["submissionId"]
            .as_str()
            .unwrap()
            .starts_with("plis1_")
    );
    assert_eq!(first["acceptedCount"], 2);
    assert_eq!(
        first["failures"],
        json!([
            {"index":1,"error":"BAD_BODY_VALUE","retryable":false},
            {"index":3,"sourceListingId":"bad","error":"BAD_BODY_VALUE","retryable":false}
        ])
    );
    let second =
        report(request(&app, &path(id), body, Some("same-key"), Some("token")).await).await;
    assert_eq!(first, second);
    let calls = sent.lock().unwrap();
    for call in calls.iter() {
        assert_eq!(
            call.iter().map(|m| m.metadata.index).collect::<Vec<_>>(),
            [0, 2]
        );
        assert!(
            call.iter()
                .all(|m| matches!(m.intent, ProductListingIngestionIntent::Upsert(_)))
        );
    }
    let ProductListingIngestionIntent::Upsert(ref command) = calls[0][0].intent else {
        unreachable!()
    };
    assert_eq!(command.listing_source_id, id);
    assert_eq!(command.source_listing_id.as_ref(), "SKU-1");
    assert!(command.title.is_none() && command.description.is_none());
    assert_eq!(
        calls[0][0].metadata.command_id,
        calls[1][0].metadata.command_id
    );
    assert_eq!(
        calls[0][1].metadata.command_id,
        calls[1][1].metadata.command_id
    );
    assert_ne!(
        calls[0][0].metadata.command_id,
        calls[0][1].metadata.command_id
    );
}

#[tokio::test]
async fn one_and_one_hundred_upserts_are_admitted_without_mutation() {
    let publisher = Publisher::default();
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let single = request(
        &app,
        &path,
        r#"[{"sourceListingId":"only"}]"#,
        None,
        Some("token"),
    )
    .await;
    assert_eq!(single.status(), StatusCode::ACCEPTED);
    assert!(single.headers().contains_key("idempotency-key"));
    assert_eq!(report(single).await["failures"], json!([]));
    let hundred = format!(
        "[{}]",
        (0..100)
            .map(|i| format!(r#"{{"sourceListingId":"sku-{i}"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let response = request(&app, &path, &hundred, None, Some("token")).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(report(response).await["acceptedCount"], 100);
    assert_eq!(sent.lock().unwrap()[1].len(), 100);
}

#[tokio::test]
async fn upsert_patch_semantics_and_codec_round_trip() {
    let publisher = Publisher::default();
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let auction_id = auction_core::AuctionId::new();
    let body = format!(
        r#"[
        {{"sourceListingId":"omitted"}},
        {{"sourceListingId":"clear","price":null,"priceEstimateMin":null,"priceEstimateMax":null,"availability":null,"url":null,"images":[],"auction":{{"auctionId":null,"lotNumber":null,"cataloguePosition":null,"timing":{{"biddingOpens":null,"scheduledCloses":null,"reportedClosedAt":null}}}}}},
        {{"sourceListingId":"set","price":{{"type":"MONETARY","amount":12000,"currency":"EUR"}},"priceEstimateMin":{{"amount":100,"currency":"EUR"}},"priceEstimateMax":{{"amount":200,"currency":"EUR"}},"availability":"IN_STOCK","url":"https://example.test/item","images":["https://example.test/a","https://example.test/a","https://example.test/b"],"auction":{{"auctionId":"{auction_id}","lotNumber":"42","cataloguePosition":7,"timing":{{"biddingOpens":"2026-08-23T08:00:00Z","scheduledCloses":"2026-08-24T12:00:00Z","reportedClosedAt":"2026-08-24T13:00:00Z"}}}}}},
        {{"sourceListingId":"on-request","price":{{"type":"ON_REQUEST"}}}}
    ]"#
    );
    let response = request(
        &app,
        &path(ListingSourceId::new()),
        &body,
        None,
        Some("token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(report(response).await["acceptedCount"], 4);
    let calls = sent.lock().unwrap();
    let commands = &calls[0];
    for message in commands {
        let encoded = product_listing_ingestion_sqs::codec::encode(message).unwrap();
        let decoded = product_listing_ingestion_sqs::codec::decode(&encoded)
            .unwrap()
            .into_intent()
            .unwrap();
        assert_eq!(decoded, message.intent);
    }
    let command = |i: usize| match &commands[i].intent {
        ProductListingIngestionIntent::Upsert(command) => command,
        _ => panic!("expected UPSERT"),
    };
    let omitted = command(0);
    assert!(matches!(omitted.price, PatchField::Unchanged));
    assert!(matches!(omitted.price_estimate_min, PatchField::Unchanged));
    assert!(matches!(omitted.price_estimate_max, PatchField::Unchanged));
    assert!(matches!(omitted.availability, PatchField::Unchanged));
    assert!(omitted.url.is_none());
    assert!(matches!(omitted.images, PatchField::Unchanged));
    assert!(matches!(omitted.auction, PatchField::Unchanged));
    let clear = command(1);
    assert!(matches!(clear.price, PatchField::Clear));
    assert!(matches!(clear.price_estimate_min, PatchField::Clear));
    assert!(matches!(clear.price_estimate_max, PatchField::Clear));
    assert!(matches!(clear.availability, PatchField::Clear));
    assert!(clear.url.is_none()); // Unlike PATCH, upsert null URL preserves the existing URL.
    assert!(matches!(&clear.images, PatchField::Set(images) if images.is_empty()));
    assert!(
        matches!(&clear.auction, PatchField::Set(a) if matches!(a.auction_id, PatchField::Clear) && matches!(a.lot_number, PatchField::Clear) && matches!(a.catalogue_position, PatchField::Clear) && matches!(a.bidding_opens, PatchField::Clear) && matches!(a.scheduled_closes, PatchField::Clear) && matches!(a.reported_closed_at, PatchField::Clear))
    );
    let set = command(2);
    assert!(matches!(&set.price, PatchField::Set(p) if p.monetary().is_some()));
    assert!(matches!(set.price_estimate_min, PatchField::Set(_)));
    assert!(matches!(set.price_estimate_max, PatchField::Set(_)));
    assert!(matches!(set.availability, PatchField::Set(_)));
    assert!(set.url.is_some());
    assert!(matches!(&set.images, PatchField::Set(images) if images.len() == 2));
    assert!(
        matches!(&set.auction, PatchField::Set(a) if matches!(a.auction_id, PatchField::Set(_)) && matches!(a.lot_number, PatchField::Set(_)) && matches!(a.catalogue_position, PatchField::Set(_)) && matches!(a.bidding_opens, PatchField::Set(_)) && matches!(a.scheduled_closes, PatchField::Set(_)) && matches!(a.reported_closed_at, PatchField::Set(_)))
    );
    assert!(matches!(&command(3).price, PatchField::Set(p) if p.is_on_request()));
}

#[tokio::test]
async fn invalid_items_are_independent_and_keep_strict_dto_parsing() {
    let publisher = Publisher::default();
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let invalid = [
        "null",
        "1",
        r#"{}"#,
        r#"{"sourceListingId":"   "}"#,
        r#"{"sourceListingId":"bad","images":null}"#,
        r#"{"sourceListingId":"bad","auction":null}"#,
        r#"{"sourceListingId":"bad","auction":{"auctionId":"invalid"}}"#,
        r#"{"sourceListingId":"bad","auction":{"timing":{"biddingOpens":"2026-08-23"}}}"#,
        r#"{"sourceListingId":"bad","price":{"type":"INVALID"}}"#,
        r#"{"sourceListingId":"bad","availability":"INVALID"}"#,
        r#"{"sourceListingId":"bad","url":"not-a-url"}"#,
        r#"{"sourceListingId":"bad","unknown":1}"#,
        r#"{"sourceListingId":"bad","price":null,"price":null}"#,
        r#"{"sourceListingId":"bad","sourceListingId":"bad"}"#,
    ];
    let body = format!("[{},{{\"sourceListingId\":\"valid\"}}]", invalid.join(","));
    let response = request(&app, &path, &body, None, Some("token")).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let data = report(response).await;
    assert_eq!(data["acceptedCount"], 1);
    assert_eq!(data["failures"].as_array().unwrap().len(), invalid.len());
    assert!(
        data["failures"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .all(|(i, f)| f["index"] == i && f["retryable"] == false)
    );
    assert_eq!(sent.lock().unwrap()[0][0].metadata.index, invalid.len());
    let all = request(
        &app,
        &path,
        &format!("[{}]", invalid.join(",")),
        None,
        Some("token"),
    )
    .await;
    assert_eq!(all.status(), StatusCode::BAD_REQUEST);
    assert_eq!(report(all).await["acceptedCount"], 0);
    assert_eq!(sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn authority_envelope_and_body_limit_reject_before_sending() {
    let publisher = Publisher::default();
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let path = path(ListingSourceId::new());
    let empty = request(&app, &path, "[]", None, Some("token")).await;
    assert_eq!(empty.status(), StatusCode::ACCEPTED);
    assert!(empty.headers().contains_key("idempotency-key"));
    assert_eq!(report(empty).await["failures"], json!([]));
    for body in [
        "",
        "  ",
        "{}",
        "[null,",
        "[{},]",
        "[{},{}]garbage",
        &format!("[{}]", vec!["null"; 101].join(",")),
    ] {
        assert_eq!(
            request(&app, &path, body, None, Some("token"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(&app, &path, "[]", None, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &path, "[]", None, Some("bad")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, &path, "[]", Some("contains space"), Some("token"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "/api/v1/listing-sources/invalid/product-listings/async",
            "[]",
            None,
            Some("token")
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &self::app(Publisher::default(), false),
            &path,
            "[]",
            None,
            Some("token")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &self::app(Publisher::default(), false),
            &path,
            "[null]",
            None,
            Some("token")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(sent.lock().unwrap().is_empty());
    let limited =
        crate::transport::with_transport_middleware(app, crate::transport::NATIVE_REQUEST_TIMEOUT);
    let oversized = request(&limited, &path, &"x".repeat(1_048_577), None, Some("token")).await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        oversized.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert!(sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn size_and_publisher_results_follow_shared_status_matrix() {
    let path = path(ListingSourceId::new());
    let size = app(
        Publisher {
            mode: Mode::Size,
            ..Default::default()
        },
        true,
    );
    let single = request(
        &size,
        &path,
        r#"[{"sourceListingId":"big"}]"#,
        Some("size-key"),
        Some("token"),
    )
    .await;
    assert_eq!(single.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(single.headers()["idempotency-key"], "size-key");
    assert_eq!(
        report(single).await["failures"][0]["error"],
        "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE"
    );
    let mixed = request(
        &size,
        &path,
        r#"[{"sourceListingId":"big"},{"sourceListingId":"okay"}]"#,
        None,
        Some("token"),
    )
    .await;
    assert_eq!(mixed.status(), StatusCode::ACCEPTED);
    assert_eq!(report(mixed).await["acceptedCount"], 1);
    let internal = app(
        Publisher {
            mode: Mode::Internal,
            ..Default::default()
        },
        true,
    );
    let response = request(
        &internal,
        &path,
        r#"[{"sourceListingId":"broken"}]"#,
        None,
        Some("token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        report(response).await["failures"][0]["error"],
        "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR"
    );

    let publisher = Publisher {
        mode: Mode::Mixed,
        ..Default::default()
    };
    let sent = publisher.sent.clone();
    let app = app(publisher, true);
    let body = r#"[null,{"sourceListingId":"same"},{"sourceListingId":"same"},{"sourceListingId":"big"},{"sourceListingId":"other"}]"#;
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
            {"index":3,"sourceListingId":"big","error":"PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE","retryable":false}
        ])
    );
    let second = report(request(&app, &path, body, Some("retry"), Some("token")).await).await;
    assert_eq!(first["failures"], second["failures"]);
    {
        let calls = sent.lock().unwrap();
        assert_eq!(
            calls[0]
                .iter()
                .map(|m| m.metadata.index)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(
            calls[0][0].metadata.command_id,
            calls[1][0].metadata.command_id
        );
        assert_ne!(
            calls[0][0].metadata.command_id,
            calls[0][1].metadata.command_id
        );
    }
    let zero = request(
        &app,
        &path,
        r#"[null,{"sourceListingId":"same"},{"sourceListingId":"same"}]"#,
        Some("zero"),
        Some("token"),
    )
    .await;
    assert_eq!(zero.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(report(zero).await["failures"].as_array().unwrap().len(), 3);
}
