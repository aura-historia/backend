//! Shopify `product_listings/*` deliveries over HTTPS. The handler authenticates the exact body
//! bytes, maps the delivery headers to the observation's provenance, and answers `200` only after
//! the ingestion command's admission is confirmed.
use crate::auth::request_metadata;
use crate::error::{
    ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, PRODUCT_LISTING_INTERNAL_ERROR,
    PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE,
};
use crate::state::ShopifyWebhooksState;
use crate::transport::PUBLICATION_REPORT_HEADROOM;
use application::operation_context::{OperationContext, Principal};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use listing_source_core::Domain;
use product_listing_ingestion_queue::with_publication_deadline;
use product_listing_normalization::SourcePayload;
use product_listing_service::ports::shopify_webhook_verifier::ShopifyWebhookVerificationError;
use product_listing_service::use_cases::commands::process_shopify_product_listing::{
    ShopifyEventProvenance, ShopifyProductListingProcessingError,
};
use product_listing_shopify::product_listing_event_kind;
use std::time::Duration;
use tracing::{Span, field, warn};

pub(crate) const HMAC_HEADER: HeaderName = HeaderName::from_static("x-shopify-hmac-sha256");
const TOPIC_HEADER: &str = "X-Shopify-Topic";
const SHOP_DOMAIN_HEADER: &str = "X-Shopify-Shop-Domain";
const WEBHOOK_ID_HEADER: &str = "X-Shopify-Webhook-Id";
const EVENT_ID_HEADER: &str = "X-Shopify-Event-Id";
const TRIGGERED_AT_HEADER: &str = "X-Shopify-Triggered-At";
const API_VERSION_HEADER: &str = "X-Shopify-API-Version";
/// Shopify abandons a delivery after five seconds; answer before then so a slow admission is
/// retried as uncertain rather than timed out mid-response.
const PROCESSING_BUDGET: Duration = Duration::from_secs(4);

/// The provider metadata of one delivery, as Shopify sends it in headers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShopifyDeliveryHeaders {
    topic: String,
    shop_domain: Option<String>,
    webhook_id: Option<String>,
    event_id: Option<String>,
    triggered_at: Option<String>,
    api_version: Option<String>,
}

#[tracing::instrument(
    name = "post_shopify_webhook",
    skip_all,
    fields(
        shopify_topic = field::Empty,
        shopify_domain = field::Empty,
        shopify_webhook_id = field::Empty,
        shopify_event_id = field::Empty,
        shopify_api_version = field::Empty,
    )
)]
pub async fn post_shopify(
    State(state): State<ShopifyWebhooksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let signatures = headers
        .get_all(HMAC_HEADER)
        .iter()
        .map(HeaderValue::as_bytes)
        .collect::<Vec<_>>();
    match state.verifier.verify(&signatures, &body) {
        Ok(()) => {}
        Err(ShopifyWebhookVerificationError::CryptographicFailure) => {
            tracing::error!("Shopify webhook signature could not be computed");
            return ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                .with_detail("Shopify webhook verification failed internally.")
                .into_response();
        }
        Err(
            ShopifyWebhookVerificationError::MalformedSignature
            | ShopifyWebhookVerificationError::InvalidSignature,
        ) => {
            return ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field("X-Shopify-Hmac-Sha256")
                .with_detail("Shopify webhook signature is invalid.")
                .into_response();
        }
    }

    let delivery = match ShopifyDeliveryHeaders::from_headers(&headers) {
        Ok(delivery) => delivery,
        Err(error) => return error.into_response(),
    };
    delivery.record(&Span::current());
    let Some(kind) = product_listing_event_kind(&delivery.topic) else {
        return StatusCode::OK.into_response();
    };
    let shop_domain = match delivery
        .shop_domain
        .as_deref()
        .map(Domain::try_from)
        .transpose()
    {
        Ok(Some(domain)) => domain,
        Ok(None) => return missing_header(SHOP_DOMAIN_HEADER).into_response(),
        Err(_) => {
            return ApiError::bad_request(BAD_HEADER_VALUE)
                .with_header_field(SHOP_DOMAIN_HEADER)
                .with_detail("Shopify shop domain is invalid.")
                .into_response();
        }
    };
    let provenance = match delivery.into_provenance() {
        Ok(provenance) => provenance,
        Err(error) => return error.into_response(),
    };
    let payload = match serde_json::from_slice(&body)
        .ok()
        .and_then(|value| SourcePayload::new(value).ok())
    {
        Some(payload) => payload,
        None => {
            return ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("Shopify webhook body must be a JSON object.")
                .into_response();
        }
    };

    let metadata = request_metadata(&headers);
    let context = OperationContext {
        principal: Principal::System,
        request_id: metadata.request_id,
        correlation_id: metadata.correlation_id,
    };
    let deadline = tokio::time::Instant::now() + PROCESSING_BUDGET;
    let admission = state
        .process
        .execute(&context, kind, shop_domain, payload, provenance);
    match tokio::time::timeout_at(
        deadline,
        with_publication_deadline(deadline - PUBLICATION_REPORT_HEADROOM, admission),
    )
    .await
    {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        Ok(Err(error)) => processing_error(error).into_response(),
        Err(_) => {
            warn!("Shopify webhook admission exceeded its processing budget");
            admission_unconfirmed().into_response()
        }
    }
}

impl ShopifyDeliveryHeaders {
    fn from_headers(headers: &HeaderMap) -> Result<Self, ApiError> {
        Ok(Self {
            topic: single_header(headers, TOPIC_HEADER)?
                .ok_or_else(|| missing_header(TOPIC_HEADER))?,
            shop_domain: single_header(headers, SHOP_DOMAIN_HEADER)?,
            webhook_id: single_header(headers, WEBHOOK_ID_HEADER)?,
            event_id: single_header(headers, EVENT_ID_HEADER)?,
            triggered_at: single_header(headers, TRIGGERED_AT_HEADER)?,
            api_version: single_header(headers, API_VERSION_HEADER)?,
        })
    }

    fn record(&self, span: &Span) {
        span.record("shopify_topic", self.topic.as_str());
        for (name, value) in [
            ("shopify_domain", &self.shop_domain),
            ("shopify_webhook_id", &self.webhook_id),
            ("shopify_event_id", &self.event_id),
            ("shopify_api_version", &self.api_version),
        ] {
            if let Some(value) = value {
                span.record(name, value.as_str());
            }
        }
    }

    /// A forwarded delivery must name its webhook delivery, which keys its deduplication.
    fn into_provenance(self) -> Result<ShopifyEventProvenance, ApiError> {
        Ok(ShopifyEventProvenance {
            topic: self.topic,
            shopify_event_id: self.event_id,
            webhook_id: self
                .webhook_id
                .ok_or_else(|| missing_header(WEBHOOK_ID_HEADER))?,
            triggered_at: self.triggered_at,
        })
    }
}

/// A present header must occur once, as visible ASCII, and not be blank.
fn single_header(headers: &HeaderMap, name: &'static str) -> Result<Option<String>, ApiError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let invalid = || {
        ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field(name)
            .with_detail("Shopify header must occur once and be nonblank visible text.")
    };
    if values.next().is_some() {
        return Err(invalid());
    }
    match value.to_str() {
        Ok(text) if !text.trim().is_empty() => Ok(Some(text.to_owned())),
        _ => Err(invalid()),
    }
}

fn missing_header(name: &'static str) -> ApiError {
    ApiError::bad_request(BAD_HEADER_VALUE)
        .with_header_field(name)
        .with_detail("Shopify header is required.")
}

fn processing_error(error: ShopifyProductListingProcessingError) -> ApiError {
    match error {
        ShopifyProductListingProcessingError::InvalidTriggeredAt(_) => {
            warn!(%error, "Shopify trigger time is unusable; rejecting the delivery");
            ApiError::bad_request(BAD_HEADER_VALUE)
                .with_header_field(TRIGGERED_AT_HEADER)
                .with_detail(
                    "Shopify trigger time must be RFC 3339 and within Shopify's retry horizon.",
                )
        }
        ShopifyProductListingProcessingError::InvalidPayload(_)
        | ShopifyProductListingProcessingError::InvalidProvenance(_)
        | ShopifyProductListingProcessingError::InvalidProviderReceipt(_) => {
            warn!(%error, "Shopify delivery cannot be mapped; rejecting it");
            ApiError::bad_request(BAD_BODY_VALUE).with_detail("Shopify delivery is invalid.")
        }
        ShopifyProductListingProcessingError::ListingSourceLookup(_)
        | ShopifyProductListingProcessingError::Submit(_)
        | ShopifyProductListingProcessingError::ForwardUnconfirmed
        | ShopifyProductListingProcessingError::InvalidSubmissionIdentity => {
            warn!(%error, "Shopify delivery admission was not confirmed");
            admission_unconfirmed()
        }
    }
}

fn admission_unconfirmed() -> ApiError {
    ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
        .with_detail("Shopify delivery admission was not confirmed; retry the delivery.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::Request;
    use base64::Engine;
    use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
    use product_listing_service::ports::shopify_product_decoder::ShopifyProductEventKind;
    use product_listing_service::ports::shopify_webhook_verifier::ShopifyWebhookVerifier;
    use product_listing_service::use_cases::ProductListingIngestionSubmissionError;
    use product_listing_service::use_cases::commands::process_shopify_product_listing::{
        ProcessShopifyProductListingUseCase, ShopifyTriggeredAtError,
    };
    use product_listing_shopify::{ShopifyHmacWebhookVerifier, ShopifyWebhookSecrets};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    const SECRET: &str = "shopify-client-secret";
    const PREVIOUS_SECRET: &str = "shopify-previous-client-secret";

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Execution {
        principal: Principal,
        kind: ShopifyProductEventKind,
        shop_domain: String,
        payload: Value,
        topic: String,
        event_id: Option<String>,
        webhook_id: String,
        triggered_at: Option<String>,
    }

    type ProcessingFailure = fn() -> ShopifyProductListingProcessingError;

    #[derive(Clone, Copy)]
    enum Outcome {
        Admitted,
        Fails(ProcessingFailure),
        Hangs,
    }

    struct FakeProcessor {
        outcome: Outcome,
        executions: Mutex<Vec<Execution>>,
    }

    impl FakeProcessor {
        fn new(outcome: Outcome) -> Arc<Self> {
            Arc::new(Self {
                outcome,
                executions: Mutex::new(Vec::new()),
            })
        }

        fn executions(&self) -> Vec<Execution> {
            self.executions.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ProcessShopifyProductListingUseCase for FakeProcessor {
        async fn execute(
            &self,
            context: &OperationContext,
            kind: ShopifyProductEventKind,
            shop_domain: Domain,
            payload: SourcePayload,
            provenance: ShopifyEventProvenance,
        ) -> Result<(), ShopifyProductListingProcessingError> {
            self.executions.lock().unwrap().push(Execution {
                principal: context.principal.clone(),
                kind,
                shop_domain: shop_domain.as_str().to_owned(),
                payload: payload.value().clone(),
                topic: provenance.topic,
                event_id: provenance.shopify_event_id,
                webhook_id: provenance.webhook_id,
                triggered_at: provenance.triggered_at,
            });
            match self.outcome {
                Outcome::Admitted => Ok(()),
                Outcome::Fails(error) => Err(error()),
                Outcome::Hangs => std::future::pending().await,
            }
        }
    }

    fn router(
        verifier: Arc<dyn ShopifyWebhookVerifier>,
        process: Arc<dyn ProcessShopifyProductListingUseCase>,
    ) -> axum::Router {
        crate::app(
            AppState::new().with_shopify_webhooks(ShopifyWebhooksState::new(verifier, process)),
        )
    }

    fn secrets() -> Arc<dyn ShopifyWebhookVerifier> {
        Arc::new(ShopifyHmacWebhookVerifier::new(
            ShopifyWebhookSecrets::new(SECRET.to_owned(), Some(PREVIOUS_SECRET.to_owned()))
                .unwrap(),
        ))
    }

    struct BrokenVerifier;

    impl ShopifyWebhookVerifier for BrokenVerifier {
        fn verify(
            &self,
            _signatures: &[&[u8]],
            _raw_body: &[u8],
        ) -> Result<(), ShopifyWebhookVerificationError> {
            Err(ShopifyWebhookVerificationError::CryptographicFailure)
        }
    }

    fn sign(secret: &str, body: &[u8]) -> String {
        let key = PKey::hmac(secret.as_bytes()).unwrap();
        let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
        signer.update(body).unwrap();
        base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
    }

    fn body() -> Vec<u8> {
        serde_json::to_vec(&json!({"product_listing": {
            "product_id": 42,
            "title": "Cabinet",
            "handle": "cabinet",
            "variants": [{"price": "42.00", "inventory_quantity": 1}],
            "images": []
        }}))
        .unwrap()
    }

    fn delivery_headers(topic: &str) -> Vec<(&'static str, String)> {
        vec![
            ("X-Shopify-Topic", topic.to_owned()),
            ("X-Shopify-Shop-Domain", "partner.example".to_owned()),
            ("X-Shopify-Webhook-Id", "webhook-1".to_owned()),
            ("X-Shopify-Event-Id", "event-1".to_owned()),
            ("X-Shopify-Triggered-At", "2026-10-27T10:00:00Z".to_owned()),
            ("X-Shopify-API-Version", "2026-10".to_owned()),
        ]
    }

    fn request(headers: &[(&str, String)], body: Vec<u8>) -> Request<Body> {
        let mut builder =
            Request::post("/v1/webhooks/shopify").header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        builder.body(Body::from(body)).unwrap()
    }

    fn signed_request(headers: Vec<(&'static str, String)>, body: Vec<u8>) -> Request<Body> {
        let mut headers = headers;
        headers.push(("X-Shopify-Hmac-Sha256", sign(SECRET, &body)));
        request(&headers, body)
    }

    async fn send(router: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    #[tokio::test]
    async fn should_admit_signed_delivery_with_header_provenance() {
        let processor = FakeProcessor::new(Outcome::Admitted);
        let (status, response) = send(
            router(secrets(), processor.clone()),
            signed_request(delivery_headers("product_listings/update"), body()),
        )
        .await;

        assert_eq!((StatusCode::OK, Value::Null), (status, response));
        assert_eq!(
            vec![Execution {
                principal: Principal::System,
                kind: ShopifyProductEventKind::Update,
                shop_domain: "partner.example".to_owned(),
                payload: serde_json::from_slice(&body()).unwrap(),
                topic: "product_listings/update".to_owned(),
                event_id: Some("event-1".to_owned()),
                webhook_id: "webhook-1".to_owned(),
                triggered_at: Some("2026-10-27T10:00:00Z".to_owned()),
            }],
            processor.executions()
        );
    }

    #[tokio::test]
    async fn should_admit_delivery_signed_with_previous_secret_and_without_optional_headers() {
        let processor = FakeProcessor::new(Outcome::Admitted);
        let body = body();
        let mut headers = delivery_headers("product_listings/remove");
        headers.retain(|(name, _)| {
            !matches!(
                *name,
                "X-Shopify-Event-Id" | "X-Shopify-Triggered-At" | "X-Shopify-API-Version"
            )
        });
        headers.push(("X-Shopify-Hmac-Sha256", sign(PREVIOUS_SECRET, &body)));

        let (status, _) = send(
            router(secrets(), processor.clone()),
            request(&headers, body),
        )
        .await;

        assert_eq!(StatusCode::OK, status);
        let executions = processor.executions();
        assert_eq!(ShopifyProductEventKind::Remove, executions[0].kind);
        assert_eq!(
            (None, None),
            (
                executions[0].event_id.clone(),
                executions[0].triggered_at.clone()
            )
        );
    }

    #[tokio::test]
    async fn should_reject_unauthenticated_delivery_before_parsing_it() {
        let body = b"not json".to_vec();
        let mut wrong_secret = delivery_headers("product_listings/add");
        wrong_secret.push(("X-Shopify-Hmac-Sha256", sign("other-secret", &body)));
        let mut tampered = delivery_headers("product_listings/add");
        tampered.push(("X-Shopify-Hmac-Sha256", sign(SECRET, b"original body")));

        for headers in [
            delivery_headers("product_listings/add"),
            wrong_secret,
            tampered,
        ] {
            let processor = FakeProcessor::new(Outcome::Admitted);
            let (status, response) = send(
                router(secrets(), processor.clone()),
                request(&headers, body.clone()),
            )
            .await;

            assert_eq!(StatusCode::UNAUTHORIZED, status);
            assert_eq!("BAD_HEADER_VALUE", response["error"]);
            assert!(processor.executions().is_empty());
        }
    }

    #[tokio::test]
    async fn should_fail_closed_when_signature_cannot_be_computed() {
        let processor = FakeProcessor::new(Outcome::Admitted);
        let (status, response) = send(
            router(Arc::new(BrokenVerifier), processor.clone()),
            signed_request(delivery_headers("product_listings/add"), body()),
        )
        .await;

        assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, status);
        assert_eq!("PRODUCT_LISTING_INTERNAL_ERROR", response["error"]);
        assert!(processor.executions().is_empty());
    }

    #[tokio::test]
    async fn should_acknowledge_other_topics_without_forwarding() {
        for topic in [
            "products/create",
            "products/update",
            "products/delete",
            "orders/create",
        ] {
            let processor = FakeProcessor::new(Outcome::Admitted);
            let mut headers = delivery_headers(topic);
            headers.retain(|(name, _)| *name != "X-Shopify-Shop-Domain");
            let (status, _) = send(
                router(secrets(), processor.clone()),
                signed_request(headers, b"[]".to_vec()),
            )
            .await;

            assert_eq!(StatusCode::OK, status, "{topic}");
            assert!(processor.executions().is_empty(), "{topic}");
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_delivery_without_forwarding() {
        let without = |header: &str| {
            let mut headers = delivery_headers("product_listings/add");
            headers.retain(|(name, _)| *name != header);
            headers
        };
        let with = |header: &'static str, value: &str| {
            let mut headers = without(header);
            headers.push((header, value.to_owned()));
            headers
        };
        let mut repeated_topic = delivery_headers("product_listings/add");
        repeated_topic.push(("X-Shopify-Topic", "product_listings/add".to_owned()));
        let cases = [
            (without("X-Shopify-Topic"), body(), "BAD_HEADER_VALUE"),
            (repeated_topic, body(), "BAD_HEADER_VALUE"),
            (without("X-Shopify-Shop-Domain"), body(), "BAD_HEADER_VALUE"),
            (
                with("X-Shopify-Shop-Domain", "not a domain"),
                body(),
                "BAD_HEADER_VALUE",
            ),
            (without("X-Shopify-Webhook-Id"), body(), "BAD_HEADER_VALUE"),
            (
                with("X-Shopify-Webhook-Id", " "),
                body(),
                "BAD_HEADER_VALUE",
            ),
            (
                delivery_headers("product_listings/add"),
                b"{".to_vec(),
                "BAD_BODY_VALUE",
            ),
            (
                delivery_headers("product_listings/add"),
                b"[]".to_vec(),
                "BAD_BODY_VALUE",
            ),
        ];

        for (index, (headers, body, error)) in cases.into_iter().enumerate() {
            let processor = FakeProcessor::new(Outcome::Admitted);
            let (status, response) = send(
                router(secrets(), processor.clone()),
                signed_request(headers, body),
            )
            .await;

            assert_eq!(
                (StatusCode::BAD_REQUEST, error),
                (status, response["error"].as_str().unwrap()),
                "case {index}"
            );
            assert!(processor.executions().is_empty(), "case {index}");
        }
    }

    #[tokio::test]
    async fn should_map_processing_outcomes_to_retryable_or_final_statuses() {
        let cases: [(ProcessingFailure, StatusCode, &str); 5] = [
            (
                || {
                    ShopifyProductListingProcessingError::InvalidPayload(
                        application::error::static_error("missing title"),
                    )
                },
                StatusCode::BAD_REQUEST,
                "BAD_BODY_VALUE",
            ),
            (
                || {
                    ShopifyProductListingProcessingError::InvalidTriggeredAt(
                        ShopifyTriggeredAtError::InFuture,
                    )
                },
                StatusCode::BAD_REQUEST,
                "BAD_HEADER_VALUE",
            ),
            (
                || ShopifyProductListingProcessingError::ForwardUnconfirmed,
                StatusCode::SERVICE_UNAVAILABLE,
                "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            ),
            (
                || {
                    ShopifyProductListingProcessingError::Submit(
                        ProductListingIngestionSubmissionError::InconsistentInputIndices,
                    )
                },
                StatusCode::SERVICE_UNAVAILABLE,
                "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            ),
            (
                || ShopifyProductListingProcessingError::InvalidSubmissionIdentity,
                StatusCode::SERVICE_UNAVAILABLE,
                "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            ),
        ];

        for (error, expected_status, expected_error) in cases {
            let (status, response) = send(
                router(secrets(), FakeProcessor::new(Outcome::Fails(error))),
                signed_request(delivery_headers("product_listings/add"), body()),
            )
            .await;

            assert_eq!(
                (expected_status, expected_error),
                (status, response["error"].as_str().unwrap())
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_answer_retryable_status_when_admission_exceeds_processing_budget() {
        let started = tokio::time::Instant::now();
        let (status, response) = send(
            router(secrets(), FakeProcessor::new(Outcome::Hangs)),
            signed_request(delivery_headers("product_listings/add"), body()),
        )
        .await;

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, status);
        assert_eq!("PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE", response["error"]);
        assert_eq!(PROCESSING_BUDGET, started.elapsed());
    }
}
