use super::{
    async_batch,
    types::{UpdateProductListingData, parse_listing_source_id},
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

pub async fn update_products(
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
    let (items, source_ids, failures) = async_batch::parse_items::<UpdateProductListingData>(
        raw_items,
        |product| &product.source_listing_id,
        |product| {
            product
                .into_key_and_command(listing_source_id)
                .map(
                    |(product_key, command)| ProductListingIngestionIntent::Update {
                        product_key,
                        command,
                    },
                )
        },
    );
    match state
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
        .await
    {
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

    #[derive(Clone, Default)]
    struct Publisher {
        sent: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
        mixed: bool,
        size: bool,
    }
    #[async_trait::async_trait]
    impl ProductListingIngestionPublisher for Publisher {
        async fn publish(
            &self,
            messages: Vec<ProductListingIngestionMessage>,
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
            let outcomes = messages.iter().map(|message| ProductListingIngestionItemOutcome {
                index: message.metadata.index,
                command_id: message.metadata.command_id.clone(),
                outcome: if self.size {
                    ProductListingIngestionOutcome::Rejected {
                        reason: ProductListingIngestionRejectionReason::Publisher { code: "INVALID_MESSAGE_SIZE".into() },
                        retryable: false,
                    }
                } else if self.mixed {
                    match message.metadata.index {
                        1 => ProductListingIngestionOutcome::Unconfirmed,
                        2 => ProductListingIngestionOutcome::NotAttempted {
                            reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                            retryable: true,
                        },
                        _ => ProductListingIngestionOutcome::Accepted,
                    }
                } else { ProductListingIngestionOutcome::Accepted },
            }).collect();
            self.sent.lock().unwrap().push(messages);
            Ok(outcomes)
        }
    }
    fn app(publisher: Publisher, writer: bool) -> Router {
        Router::new()
            .route(
                "/api/v1/listing-sources/{listing_source_id}/product-listings/async",
                post(crate::partner_product_listings::async_create_products::create_products)
                    .patch(update_products),
            )
            .with_state(AsyncPartnerProductListingsState::new(
                Arc::new(SubmitPartnerProductListingIngestionHandler::new(publisher)),
                Arc::new(Auth {
                    writer,
                    user_id: UserId::new(),
                }),
            ))
    }
    async fn request(
        app: &Router,
        path: &str,
        body: &str,
        key: Option<&str>,
        authenticated: bool,
    ) -> Response {
        let mut request = Request::builder().method("PATCH").uri(path);
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
    async fn report(response: Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }
    fn path(id: ListingSourceId) -> String {
        format!("/api/v1/listing-sources/{id}/product-listings/async")
    }

    #[tokio::test]
    async fn partial_admission_preserves_sparse_indices_and_repeated_keys_on_retry() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let path = path(ListingSourceId::new());
        let body = r#"[{"sourceListingId":"repeat"},null,{"sourceListingId":"repeat","images":[]},{"sourceListingId":"invalid","url":null}]"#;
        let first = request(&app, &path, body, Some("same-key"), true).await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        assert_eq!(first.headers()["idempotency-key"], "same-key");
        let first = report(first).await;
        let second = report(request(&app, &path, body, Some("same-key"), true).await).await;
        assert_eq!(first, second);
        assert_eq!(first["acceptedCount"], 2);
        assert_eq!(
            first["failures"],
            json!([
                {"index":1,"error":"BAD_BODY_VALUE","retryable":false},
                {"index":3,"sourceListingId":"invalid","error":"BAD_BODY_VALUE","retryable":false}
            ])
        );
        let calls = sent.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for call in calls.iter() {
            assert_eq!(
                call.iter().map(|m| m.metadata.index).collect::<Vec<_>>(),
                [0, 2]
            );
            assert!(
                call.iter()
                    .all(|m| matches!(m.intent, ProductListingIngestionIntent::Update { .. }))
            );
        }
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
    async fn patch_values_reach_publisher_without_loading_target_or_auction() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let id = ListingSourceId::new();
        let body = format!(
            r#"[
            {{"sourceListingId":"missing"}},
            {{"sourceListingId":"withdrawn","price":null,"priceEstimateMin":null,"priceEstimateMax":null,"availability":null,"images":[]}},
            {{"sourceListingId":"auction","price":{{"type":"ON_REQUEST"}},"priceEstimateMin":{{"amount":100,"currency":"EUR"}},"priceEstimateMax":{{"amount":200,"currency":"EUR"}},"availability":"AVAILABLE","url":"https://example.test/new","images":["https://example.test/image"],"auction":{{"auctionId":"{}","lotNumber":"42","cataloguePosition":7,"timing":{{"biddingOpens":"2026-08-23T08:00:00Z","scheduledCloses":null}}}}}},
            {{"sourceListingId":"leaves","price":{{"type":"MONETARY","amount":12000,"currency":"EUR"}},"auction":{{"auctionId":null,"lotNumber":null,"cataloguePosition":null,"timing":{{"reportedClosedAt":null}}}}}}
        ]"#,
            auction_core::AuctionId::new()
        );
        let response = request(&app, &path(id), &body, None, true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(report(response).await["acceptedCount"], 4);
        let calls = sent.lock().unwrap();
        let messages = &calls[0];
        for message in messages {
            let wire = product_listing_ingestion_sqs::codec::encode(message).unwrap();
            let decoded = product_listing_ingestion_sqs::codec::decode(&wire).unwrap();
            let ProductListingIngestionIntent::Update { command, .. } =
                decoded.into_intent().unwrap()
            else {
                panic!("codec must preserve UPDATE");
            };
            let ProductListingIngestionIntent::Update {
                command: original, ..
            } = &message.intent
            else {
                unreachable!();
            };
            assert_eq!(command, *original);
        }
        let update = |index: usize| match &messages[index].intent {
            ProductListingIngestionIntent::Update {
                command,
                product_key,
            } => {
                assert_eq!(product_key.listing_source_id, id);
                command
            }
            _ => panic!("expected UPDATE"),
        };
        let omitted = update(0);
        assert!(matches!(omitted.price, PatchField::Unchanged));
        assert!(matches!(omitted.price_estimate_min, PatchField::Unchanged));
        assert!(matches!(omitted.price_estimate_max, PatchField::Unchanged));
        assert!(matches!(omitted.availability, PatchField::Unchanged));
        assert!(matches!(omitted.url, PatchField::Unchanged));
        assert!(matches!(omitted.images, PatchField::Unchanged));
        assert!(matches!(omitted.auction, PatchField::Unchanged));
        let clear = update(1);
        assert!(matches!(clear.price, PatchField::Clear));
        assert!(matches!(clear.price_estimate_min, PatchField::Clear));
        assert!(matches!(clear.price_estimate_max, PatchField::Clear));
        assert!(matches!(clear.availability, PatchField::Clear));
        assert!(matches!(&clear.images, PatchField::Set(images) if images.is_empty()));
        let set = update(2);
        assert!(matches!(&set.price, PatchField::Set(price) if price.is_on_request()));
        assert!(matches!(set.price_estimate_min, PatchField::Set(_)));
        assert!(matches!(set.price_estimate_max, PatchField::Set(_)));
        assert!(matches!(set.availability, PatchField::Set(_)));
        assert!(matches!(set.url, PatchField::Set(_)));
        assert!(matches!(&set.images, PatchField::Set(images) if images.len() == 1));
        assert!(
            matches!(&set.auction, PatchField::Set(a) if matches!(a.auction_id, PatchField::Set(_))
            && matches!(a.lot_number, PatchField::Set(_))
            && matches!(a.catalogue_position, PatchField::Set(_))
            && matches!(a.bidding_opens, PatchField::Set(_))
            && matches!(a.scheduled_closes, PatchField::Clear)
            && matches!(a.reported_closed_at, PatchField::Unchanged))
        );
        assert!(matches!(&update(3).price, PatchField::Set(price) if price.monetary().is_some()));
        assert!(
            matches!(&update(3).auction, PatchField::Set(a) if matches!(a.auction_id, PatchField::Clear)
            && matches!(a.lot_number, PatchField::Clear)
            && matches!(a.catalogue_position, PatchField::Clear)
            && matches!(a.reported_closed_at, PatchField::Clear))
        );
    }

    #[tokio::test]
    async fn invalid_items_fail_independently_and_strictly() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let path = path(ListingSourceId::new());
        let invalid = [
            r#"{"sourceListingId":"url","url":null}"#,
            r#"{"sourceListingId":"images","images":null}"#,
            r#"{"sourceListingId":"auction","auction":null}"#,
            r#"{"sourceListingId":"availability","availability":"NOT_A_STATUS"}"#,
            r#"{"sourceListingId":"unknown","title":"not allowed"}"#,
            r#"{"sourceListingId":"duplicate","price":null,"price":null}"#,
            r#"{"sourceListingId":"bad-auction","auction":{"auctionId":"invalid"}}"#,
            r#"{"sourceListingId":"lot","auction":{"lotNumber":" "}}"#,
            r#"{"sourceListingId":"position","auction":{"cataloguePosition":0}}"#,
            r#"{"sourceListingId":"time","auction":{"timing":{"biddingOpens":"2026-08-23"}}}"#,
            r#"{"sourceListingId":"bad-price","price":{"type":"INVALID"}}"#,
            r#"{"sourceListingId":"missing","sourceListingId":"missing"}"#,
            r#"{"sourceListingId":"blank","url":"not-a-url"}"#,
            r#"{"sourceListingId":" "}"#,
            "null",
            "123",
        ];
        let body = format!("[{},{{\"sourceListingId\":\"valid\"}}]", invalid.join(","));
        let response = request(&app, &path, &body, Some("mixed"), true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let result = report(response).await;
        assert_eq!(result["acceptedCount"], 1);
        assert_eq!(result["failures"].as_array().unwrap().len(), invalid.len());
        assert_eq!(
            *sent.lock().unwrap()[0]
                .iter()
                .map(|m| m.metadata.index)
                .collect::<Vec<_>>()
                .first()
                .unwrap(),
            invalid.len()
        );
        assert!(
            result["failures"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .all(|(index, f)| f["index"] == index && f["retryable"] == false)
        );
        let all = request(&app, &path, &format!("[{}]", invalid.join(",")), None, true).await;
        assert_eq!(all.status(), StatusCode::BAD_REQUEST);
        assert_eq!(report(all).await["acceptedCount"], 0);
        assert_eq!(sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn limits_authority_and_evaluated_statuses() {
        let publisher = Publisher::default();
        let sent = publisher.sent.clone();
        let app = app(publisher.clone(), true);
        let path = path(ListingSourceId::new());
        let empty = request(&app, &path, "[]", None, true).await;
        assert_eq!(empty.status(), StatusCode::ACCEPTED);
        assert!(empty.headers().contains_key("idempotency-key"));
        assert_eq!(report(empty).await["failures"], json!([]));
        let body = format!("[{}]", vec![r#"{"sourceListingId":"same"}"#; 100].join(","));
        let response = request(&app, &path, &body, None, true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(report(response).await["acceptedCount"], 100);
        for body in [
            "",
            "{}",
            "[null,",
            "[{},]",
            &format!("[{}]", vec!["null"; 101].join(",")),
        ] {
            assert_eq!(
                request(&app, &path, body, None, true).await.status(),
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            request(&app, &path, "[]", None, false).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let invalid_bearer = Request::builder()
            .method("PATCH")
            .uri(&path)
            .header(header::AUTHORIZATION, "Bearer invalid")
            .body(Body::from("[null]"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(invalid_bearer).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, &path, "[]", Some("bad key"), true)
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &app,
                "/api/v1/listing-sources/bad/product-listings/async",
                "[]",
                None,
                true
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(
                &self::app(publisher.clone(), false),
                &path,
                "[null]",
                None,
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &self::app(publisher.clone(), false),
                &path,
                "[]",
                None,
                true
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(sent.lock().unwrap().len(), 1);
        let limited = crate::transport::with_transport_middleware(
            app,
            crate::transport::NATIVE_REQUEST_TIMEOUT,
        );
        let oversized = request(&limited, &path, &"x".repeat(1_048_577), None, true).await;
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            oversized.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn publisher_results_keep_confirmed_and_blocked_distinct() {
        let publisher = Publisher {
            mixed: true,
            ..Default::default()
        };
        let sent = publisher.sent.clone();
        let app = app(publisher, true);
        let path = path(ListingSourceId::new());
        let body = r#"[null,{"sourceListingId":"same"},{"sourceListingId":"same"},{"sourceListingId":"other"}]"#;
        let response = request(&app, &path, body, Some("retry"), true).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let first = report(response).await;
        let retry = report(request(&app, &path, body, Some("retry"), true).await).await;
        assert_eq!(first, retry);
        assert_eq!(first["acceptedCount"], 1);
        assert_eq!(
            first["failures"],
            json!([
                {"index":0,"error":"BAD_BODY_VALUE","retryable":false},
                {"index":1,"sourceListingId":"same","error":"ENQUEUE_UNCONFIRMED","retryable":true},
                {"index":2,"sourceListingId":"same","error":"ENQUEUE_BLOCKED","retryable":true}
            ])
        );
        assert_eq!(sent.lock().unwrap().len(), 2);
        let all_size = self::app(
            Publisher {
                size: true,
                ..Default::default()
            },
            true,
        );
        let size = request(
            &all_size,
            &path,
            r#"[{"sourceListingId":"large"}]"#,
            None,
            true,
        )
        .await;
        assert_eq!(size.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            report(size).await["failures"][0]["error"],
            "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE"
        );
        let mixed = request(
            &all_size,
            &path,
            r#"[null,{"sourceListingId":"large"}]"#,
            None,
            true,
        )
        .await;
        assert_eq!(mixed.status(), StatusCode::BAD_REQUEST);
    }
}
