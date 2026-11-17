//! WooCommerce deadline regression through the HTTP router and its request timeout, the real
//! intake service, partner submission handler, and scoped SQS SDK publisher. Only external
//! ports are faked.
use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app_with_request_timeout,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, WebhooksState},
};
use aws_sdk_sqs::config::{Credentials, Region};
use aws_smithy_runtime_api::{
    client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpConnector,
        },
        orchestrator::{HttpRequest, HttpResponse},
        result::ConnectorError,
        runtime_components::RuntimeComponents,
    },
    shared::IntoShared,
};
use axum::{Router, body::to_bytes, http::StatusCode};
use base64::Engine;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
    WoocommerceSource, WoocommerceSourceReader,
};
use localization::Language;
use money::Currency;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use product_listing_ingestion_sqs::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher, codec,
};
use product_listing_service::use_cases::{
    AuthorizeProductListingRawCaptureError, AuthorizeProductListingRawCaptureRequest,
    AuthorizeProductListingRawCaptureResult, AuthorizeProductListingRawCaptureUseCase,
    ProductListingIngestionActor, ProductListingIngestionIntent,
    SubmitPartnerProductListingIngestionHandler,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tower::ServiceExt;
use user_core::user_id::UserId;
use woocommerce_service::WoocommerceWebhookIntake;

const QUEUE_URL: &str = "https://sqs.us-east-1.amazonaws.com/123456789012/ingestion.fifo";
const SOURCE_ID: &str = "ls_01h455vb4pex5vy7enb1p677vn";
const SECRET: &str = "woocommerce-deadline-test-secret";
const RAW_BODY: &[u8] = br#"{ "id": 901, "name": "Lambda \u00e9 cabinet", "permalink": "https://partner.example/901", "price": "42.00", "status": "publish", "stock_status": "instock", "images": [], "futureWooKey": "\u00e9" }"#;
const DELIVERY_ID: &str = "deadline-delivery-901";
const CORRELATION_ID: &str = "woocommerce-deadline-correlation";
/// Publication stops 300ms before the request times out.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

struct Auth {
    user_id: UserId,
}

#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        if token != "partner-token" {
            return Err(AuthError::InvalidCredentials);
        }
        assert_eq!(metadata.correlation_id.as_str(), CORRELATION_ID);
        Ok(TransportPrincipal::User {
            user_id: self.user_id,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
        })
    }
}

struct SourceGate {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

struct SourceReader {
    source_id: ListingSourceId,
    gate: Arc<SourceGate>,
}

#[async_trait::async_trait]
impl WoocommerceSourceReader for SourceReader {
    async fn find_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
        assert_eq!(id, self.source_id);
        self.gate
            .entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        let release = self.gate.release.lock().unwrap().take().unwrap();
        release.await.expect("source lookup must be released");
        Ok(Some(WoocommerceSource {
            listing_source_id: id,
            currency: Some(Currency::Eur),
            language: Some(Language::En),
        }))
    }
}

struct SignatureVerifier {
    source_id: ListingSourceId,
    verified: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait::async_trait]
impl WoocommerceSignatureVerifier for SignatureVerifier {
    async fn verify(
        &self,
        id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError> {
        assert_eq!(id, self.source_id);
        self.verified.lock().unwrap().push(body.to_vec());
        let expected = sign(body);
        Ok(if signature == expected {
            WoocommerceSignatureVerification::Valid
        } else {
            WoocommerceSignatureVerification::Invalid
        })
    }
}

// The mapped webhook uses the submit handler, not the ignored-status authorization path.
struct UnexpectedIgnoredStatusAuthorization;

#[async_trait::async_trait]
impl AuthorizeProductListingRawCaptureUseCase for UnexpectedIgnoredStatusAuthorization {
    async fn execute(
        &self,
        _: &application::operation_context::OperationContext,
        _: AuthorizeProductListingRawCaptureRequest,
    ) -> Result<AuthorizeProductListingRawCaptureResult, AuthorizeProductListingRawCaptureError>
    {
        panic!("mapped observation must use the partner submission handler");
    }
}

// The request still traverses SDK serialization and signing; no SQS receipt is ever returned.
#[derive(Clone, Debug)]
struct PendingSqsHttp {
    requests: mpsc::UnboundedSender<Value>,
}

impl HttpConnector for PendingSqsHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri(), "https://sqs.us-east-1.amazonaws.com/");
        assert_eq!(
            request.headers().get("x-amz-target"),
            Some("AmazonSQS.SendMessageBatch")
        );
        assert!(request.headers().get("authorization").is_some());
        let body: Value = serde_json::from_slice(request.body().bytes().unwrap()).unwrap();
        assert_eq!(body["QueueUrl"], QUEUE_URL);
        self.requests
            .send(body)
            .expect("test must observe all SDK calls");
        HttpConnectorFuture::new(async {
            std::future::pending::<Result<HttpResponse, ConnectorError>>().await
        })
    }
}

impl HttpClient for PendingSqsHttp {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.clone().into_shared()
    }
}

fn sign(body: &[u8]) -> Vec<u8> {
    let key = PKey::hmac(SECRET.as_bytes()).unwrap();
    let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
    signer.update(body).unwrap();
    signer.sign_to_vec().unwrap()
}

fn router(
    source_id: ListingSourceId,
    user_id: UserId,
    gate: Arc<SourceGate>,
    verified: Arc<Mutex<Vec<Vec<u8>>>>,
    requests: mpsc::UnboundedSender<Value>,
) -> Router {
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "test",
            "test",
            None,
            None,
            "deadline-test",
        ))
        .http_client(PendingSqsHttp { requests })
        .build();
    let publisher = ScopedSqsProductListingIngestionPublisher::new(
        SqsProductListingIngestionPublisher::new(aws_sdk_sqs::Client::from_conf(config), QUEUE_URL),
    );
    let intake = WoocommerceWebhookIntake::new(
        SourceReader { source_id, gate },
        SignatureVerifier {
            source_id,
            verified,
        },
        SubmitPartnerProductListingIngestionHandler::new(publisher),
        UnexpectedIgnoredStatusAuthorization,
    );
    app_with_request_timeout(
        AppState::new().with_webhooks(WebhooksState::new(
            Arc::new(intake),
            Arc::new(Auth { user_id }),
        )),
        REQUEST_TIMEOUT,
    )
}

fn request() -> axum::http::Request<axum::body::Body> {
    axum::http::Request::post(format!("/v1/webhooks/woocommerce/{SOURCE_ID}"))
        .header("authorization", "Bearer partner-token")
        .header("content-type", "application/json")
        .header("host", "api.example.test")
        .header("x-correlation-id", CORRELATION_ID)
        .header("x-wc-webhook-topic", "product.updated")
        .header(
            "x-wc-webhook-signature",
            base64::engine::general_purpose::STANDARD.encode(sign(RAW_BODY)),
        )
        .header("x-wc-webhook-delivery-id", DELIVERY_ID)
        .body(axum::body::Body::from(RAW_BODY.to_vec()))
        .unwrap()
}

type TestSetup = (
    Router,
    UserId,
    Arc<Mutex<Vec<Vec<u8>>>>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    mpsc::UnboundedReceiver<Value>,
);

fn setup() -> TestSetup {
    let source_id = ListingSourceId::try_from(SOURCE_ID).unwrap();
    let user_id = UserId::new();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(SourceGate {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let verified = Arc::new(Mutex::new(Vec::new()));
    let (calls_tx, calls_rx) = mpsc::unbounded_channel();
    (
        router(source_id, user_id, gate, Arc::clone(&verified), calls_tx),
        user_id,
        verified,
        entered_rx,
        release_tx,
        calls_rx,
    )
}

async fn assert_unavailable(response: axum::response::Response) -> String {
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let request_id = response
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        response.headers().get("x-correlation-id").unwrap(),
        CORRELATION_ID
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let problem: Value = serde_json::from_slice(&body).expect("503 must have an API error body");
    assert_eq!(problem["error"], "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE");
    request_id
}

#[tokio::test(start_paused = true)]
async fn delayed_woocommerce_lookup_and_pending_sdk_send_return_503_before_the_request_timeout() {
    let (app, user_id, verified, entered, release, mut calls) = setup();
    let response = tokio::spawn(app.oneshot(request()));
    entered.await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(
        calls.try_recv().is_err(),
        "source lookup must precede publication"
    );
    assert!(verified.lock().unwrap().is_empty());
    release.send(()).unwrap();

    let call = calls
        .recv()
        .await
        .expect("mapped webhook must reach the SDK");
    let entries = call["Entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let wire = entries[0]["MessageBody"].as_str().unwrap();
    let message = codec::decode(wire).unwrap();
    assert_eq!(
        message.actor().unwrap(),
        ProductListingIngestionActor::DelegatedUser(user_id)
    );
    assert_eq!(message.correlation_id().as_str(), CORRELATION_ID);
    let wire_request_id = message.request_id();
    assert_eq!(wire_request_id.as_str().len(), 36);
    assert_eq!(message.operation().as_str(), "CAPTURE_RAW");
    let envelope = message.into_service_envelope().unwrap();
    let ProductListingIngestionIntent::CaptureRaw(command) = envelope.message.intent else {
        panic!("expected WooCommerce raw capture intent");
    };
    assert_eq!(command.source_record_key, "901");
    assert_eq!(command.source_event_id.as_deref(), Some(DELIVERY_ID));
    assert_eq!(command.input.source_payload().value()["futureWooKey"], "é");
    assert_eq!(verified.lock().unwrap().as_slice(), &[RAW_BODY.to_vec()]);
    assert!(
        !response.is_finished(),
        "pending SDK request cannot acknowledge custody"
    );

    // 4s request timeout, minus 300ms for error reporting.
    tokio::time::advance(Duration::from_millis(1750)).await;
    let response_request_id = assert_unavailable(response.await.unwrap().unwrap()).await;
    assert_eq!(wire_request_id.as_str(), response_request_id);
    assert!(
        calls.try_recv().is_err(),
        "no second send after publication deadline"
    );
}

#[tokio::test(start_paused = true)]
async fn source_lookup_finishing_after_publication_deadline_does_not_start_an_sqs_send() {
    let (app, _, verified, entered, release, mut calls) = setup();
    let response = tokio::spawn(app.oneshot(request()));
    entered.await.unwrap();
    tokio::time::advance(Duration::from_millis(3750)).await;
    assert!(calls.try_recv().is_err());
    release.send(()).unwrap();
    assert_unavailable(response.await.unwrap().unwrap()).await;
    assert_eq!(verified.lock().unwrap().as_slice(), &[RAW_BODY.to_vec()]);
    assert!(
        calls.try_recv().is_err(),
        "expired publication must not dispatch"
    );
}
