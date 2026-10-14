use application::{operation_context::CredentialCapability, patch_field::PatchField};
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    partner_product_listings::delivery::ProductListingDeliveryState,
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_ingestion_sqs::delivery::encode_command;
use product_listing_service::{
    ports::{
        ProductListingIngestionPublishError, ProductListingIngestionPublisher,
        ProductListingStorageVersion,
    },
    use_cases::*,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
struct Auth;
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        if token != "synthetic-write" && token != "synthetic-read" {
            return Err(AuthError::InvalidCredentials);
        }
        Ok(TransportPrincipal::User {
            user_id: "usr_01h455vb4pex5vy7enb1p677v4".parse().unwrap(),
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: if token == "synthetic-write" {
                [
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]
                .into()
            } else {
                [CredentialCapability::ProductListingsRead].into()
            },
        })
    }
}
#[derive(Default, Clone)]
struct Commands(Arc<Mutex<Vec<ChangeProductListingDeliveryCommand>>>);
#[async_trait::async_trait]
impl ChangeProductListingDeliveryUseCase for Commands {
    async fn execute(
        &self,
        ctx: &application::operation_context::OperationContext,
        c: ChangeProductListingDeliveryCommand,
    ) -> Result<ChangeProductListingDeliveryResult, ProductListingDeliveryError> {
        ctx.require_credential_capability(CredentialCapability::ProductListingsWrite)
            .map_err(|_| ProductListingDeliveryError::Forbidden)?;
        self.0.lock().unwrap().push(c);
        Ok(ChangeProductListingDeliveryResult {
            product_listing_id: ProductListingId::new(),
            version: ProductListingStorageVersion::try_from(2u64).unwrap(),
            outcome: domain_primitives::change_outcome::ChangeOutcome::Changed,
        })
    }
}
struct Reader;
#[async_trait::async_trait]
impl GetProductListingDeliveryUseCase for Reader {
    async fn execute(
        &self,
        _: &application::operation_context::OperationContext,
        _: ProductListingKey,
    ) -> Result<ProductListingDeliveryView, ProductListingDeliveryError> {
        Ok(ProductListingDeliveryView {
            product_listing_id: ProductListingId::new(),
            version: ProductListingStorageVersion::INITIAL,
            delivery: None,
        })
    }
}
#[derive(Default, Clone)]
struct Publisher(Arc<Mutex<Vec<ProductListingIngestionMessage>>>);
#[async_trait::async_trait]
impl ProductListingIngestionPublisher for Publisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let result = messages
            .iter()
            .map(|m| ProductListingIngestionItemOutcome {
                index: m.metadata.index,
                command_id: m.metadata.command_id.clone(),
                outcome: ProductListingIngestionOutcome::Accepted,
            })
            .collect();
        self.0.lock().unwrap().extend(messages);
        Ok(result)
    }
}
async fn request(
    router: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "synthetic-delivery-retry");
    if let Some(t) = token {
        request = request.header("authorization", format!("Bearer {t}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("no-store", response.headers().get("cache-control").unwrap());
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn api_sync_and_async_share_typed_semantics_retry_ids_and_protected_errors() {
    let commands = Commands::default();
    let publisher = Publisher::default();
    let router = app(
        AppState::new().with_product_listing_delivery(ProductListingDeliveryState {
            change: Arc::new(commands.clone()),
            get: Arc::new(Reader),
            submit: Arc::new(SubmitPartnerProductListingIngestionHandler::new(
                publisher.clone(),
            )),
            authenticator: Arc::new(Auth),
        }),
    );
    let source = ListingSourceId::new();
    let command = ChangeProductListingDeliveryCommand {
        product_key: ProductListingKey::new(source, "painting".try_into().unwrap()),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            shipping: PatchField::Set(shipping_core::selection::ComponentOverride::Replace(
                shipping_core::terms::ShippingPolicy::Prohibited,
            )),
            ..Default::default()
        }),
    };
    let body = encode_command(&command).unwrap();
    let path =
        format!("/api/v1/listing-sources/{source}/product-listings/by-source-id/painting/delivery");
    let (status, view) = request(&router, "GET", &path, Some("synthetic-read"), Value::Null).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(Value::Null, view["delivery"]);
    let (status, result) = request(
        &router,
        "PATCH",
        &path,
        Some("synthetic-write"),
        body.clone(),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(serde_json::json!(2), result["expectedVersion"]);
    assert_eq!(serde_json::json!("CHANGED"), result["outcome"]);
    let (status, first) = request(
        &router,
        "POST",
        &format!("{path}/async"),
        Some("synthetic-write"),
        body.clone(),
    )
    .await;
    assert_eq!(StatusCode::ACCEPTED, status);
    let (status, retry) = request(
        &router,
        "POST",
        &format!("{path}/async"),
        Some("synthetic-write"),
        body.clone(),
    )
    .await;
    assert_eq!(StatusCode::ACCEPTED, status);
    assert_eq!(first, retry);
    let captured = publisher.0.lock().unwrap().clone();
    assert_eq!(2, captured.len());
    assert_eq!(
        captured[0].metadata.command_id,
        captured[1].metadata.command_id
    );
    assert_eq!(
        ProductListingIngestionIntent::ChangeDelivery(command.clone()),
        captured[0].intent
    );
    assert_eq!(vec![command], *commands.0.lock().unwrap());
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, "PATCH", &path, None, body.clone()).await.0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(
            &router,
            "POST",
            &format!("{path}/async"),
            Some("synthetic-read"),
            body.clone()
        )
        .await
        .0
    );
    let mut invalid = body;
    invalid["price"] = serde_json::json!(100);
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(&router, "PATCH", &path, Some("synthetic-write"), invalid)
            .await
            .0
    );
    assert_eq!(1, commands.0.lock().unwrap().len());
    assert_eq!(2, publisher.0.lock().unwrap().len());
}

struct FailurePublisher(ProductListingIngestionOutcome);

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for FailurePublisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        Ok(messages
            .into_iter()
            .map(|message| ProductListingIngestionItemOutcome {
                index: message.metadata.index,
                command_id: message.metadata.command_id,
                outcome: self.0.clone(),
            })
            .collect())
    }
}

#[tokio::test]
async fn async_failures_retain_admission_reports_and_bound_echoed_listing_identity() {
    let source = ListingSourceId::new();
    let listing = "x".repeat(150);
    let command = ChangeProductListingDeliveryCommand {
        product_key: ProductListingKey::new(source, listing.as_str().try_into().unwrap()),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Unchanged,
    };
    for (outcome, status, retryable) in [
        (
            ProductListingIngestionOutcome::Unconfirmed,
            StatusCode::SERVICE_UNAVAILABLE,
            true,
        ),
        (
            ProductListingIngestionOutcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "INVALID_MESSAGE_SIZE".into(),
                },
                retryable: false,
            },
            StatusCode::PAYLOAD_TOO_LARGE,
            false,
        ),
        (
            ProductListingIngestionOutcome::Rejected {
                reason: ProductListingIngestionRejectionReason::Publisher {
                    code: "SQS_SENDER_FAILURE".into(),
                },
                retryable: false,
            },
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
        ),
    ] {
        let router = app(AppState::new().with_product_listing_delivery(
            ProductListingDeliveryState {
                change: Arc::new(Commands::default()),
                get: Arc::new(Reader),
                submit: Arc::new(SubmitPartnerProductListingIngestionHandler::new(
                    FailurePublisher(outcome),
                )),
                authenticator: Arc::new(Auth),
            },
        ));
        let (actual_status, report) = request(
            &router,
            "POST",
            &format!("/api/v1/listing-sources/{source}/product-listings/by-source-id/{listing}/delivery/async"),
            Some("synthetic-write"),
            encode_command(&command).unwrap(),
        ).await;
        assert_eq!(status, actual_status);
        assert_eq!(0, report["acceptedCount"]);
        assert_eq!(retryable, report["failures"][0]["retryable"]);
        assert!(report["failures"][0].get("sourceListingId").is_none());
    }
}
