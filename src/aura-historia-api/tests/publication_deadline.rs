//! Deadline regression across the HTTP router and its request timeout, the shared HTTP
//! middleware, partner admission, and the real SQS SDK publisher. The connector replaces only
//! the network, not the SDK.
use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app_with_request_timeout,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, AsyncPartnerProductListingsState},
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
use aws_smithy_types::body::SdkBody;
use axum::{Router, body::to_bytes, http::StatusCode};
use product_listing_ingestion_sqs::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher,
};
use product_listing_service::use_cases::SubmitPartnerProductListingIngestionHandler;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tower::ServiceExt;
use user_core::user_id::UserId;

const QUEUE_URL: &str = "https://sqs.us-east-1.amazonaws.com/123456789012/ingestion.fifo";
const SOURCE_ID: &str = "ls_01h455vb4pex5vy7enb1p677vn";
const PATH: &str = "/v1/listing-sources/ls_01h455vb4pex5vy7enb1p677vn/product-listings/async";
/// Publication stops 300ms before the request times out.
const TIMEOUT: Duration = Duration::from_secs(4);

struct Auth {
    gate: Option<Arc<Gate>>,
}

struct Gate {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        if token != "partner-token" {
            return Err(AuthError::InvalidCredentials);
        }
        if let Some(gate) = &self.gate {
            let release = gate.release.lock().unwrap().take();
            if let Some(release) = release {
                gate.entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(())
                    .unwrap();
                release
                    .await
                    .map_err(|_| AuthError::TemporarilyUnavailable)?;
            }
        }
        Ok(TransportPrincipal::User {
            user_id: UserId::try_from(
                uuid::Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f75").unwrap(),
            )
            .unwrap(),
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
        })
    }
}

// Every request goes through SQS request building/signing, wire serialization and receipt parsing.
// Only the HTTP connector is fake: index 2 and listing `stall` never return a receipt.
#[derive(Clone, Debug)]
struct SqsHttp {
    requests: mpsc::UnboundedSender<Value>,
}

impl HttpConnector for SqsHttp {
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
        let entries = body["Entries"].as_array().unwrap();
        let stalled = entries.iter().any(|entry| {
            let message: Value =
                serde_json::from_str(entry["MessageBody"].as_str().unwrap()).unwrap();
            message["index"] == 2 || message["payload"]["command"]["sourceListingId"] == "stall"
        });
        self.requests
            .send(body.clone())
            .expect("test must observe all SDK calls");
        if stalled {
            return HttpConnectorFuture::new(async {
                std::future::pending::<Result<HttpResponse, ConnectorError>>().await
            });
        }
        let successful: Vec<_> = entries.iter().map(|entry| {
            json!({"Id": entry["Id"], "MessageId": "confirmed", "MD5OfMessageBody": "0123456789abcdef0123456789abcdef"})
        }).collect();
        let response: HttpResponse = http::Response::builder()
            .status(200)
            .header("content-type", "application/x-amz-json-1.0")
            .body(SdkBody::from(
                json!({"Successful": successful, "Failed": []}).to_string(),
            ))
            .unwrap()
            .try_into()
            .unwrap();
        HttpConnectorFuture::ready(Ok(response))
    }
}

impl HttpClient for SqsHttp {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.clone().into_shared()
    }
}

/// One warm composition: the same state, publisher and SQS client serve every request.
fn state(requests: mpsc::UnboundedSender<Value>, gate: Option<Arc<Gate>>) -> AppState {
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
        .http_client(SqsHttp { requests })
        .build();
    let publisher = ScopedSqsProductListingIngestionPublisher::new(
        SqsProductListingIngestionPublisher::new(aws_sdk_sqs::Client::from_conf(config), QUEUE_URL),
    );
    let state = AsyncPartnerProductListingsState::new(
        Arc::new(SubmitPartnerProductListingIngestionHandler::new(publisher)),
        Arc::new(Auth { gate }),
    );
    AppState::new().with_async_partner_product_listings(state)
}

/// The router over `state` whose requests time out after `timeout`.
fn router(state: &AppState, timeout: Duration) -> Router {
    app_with_request_timeout(state.clone(), timeout)
}

fn request(body: &str, key: Option<&str>) -> axum::http::Request<axum::body::Body> {
    let mut request = axum::http::Request::delete(PATH)
        .header("authorization", "Bearer partner-token")
        .header("content-type", "application/json")
        .header("host", "api.example.test")
        .header("origin", "https://client.example.test");
    if let Some(key) = key {
        request = request.header("idempotency-key", key);
    }
    request
        .body(axum::body::Body::from(body.to_owned()))
        .unwrap()
}

/// Sends one request through a router whose requests time out after `timeout`.
async fn send(
    state: &AppState,
    body: &str,
    key: Option<&str>,
    timeout: Duration,
) -> axum::response::Response {
    router(state, timeout)
        .oneshot(request(body, key))
        .await
        .unwrap()
}

async fn report(response: axum::response::Response) -> (StatusCode, String, Value) {
    let status = response.status();
    let key = response
        .headers()
        .get("idempotency-key")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let content_type = response.headers().get("content-type").unwrap();
    assert!(
        content_type
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let body =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    (status, key, body)
}

fn wire_index(request: &Value) -> usize {
    let entries = request["Entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "same-group sends must be sequential");
    let message: Value = serde_json::from_str(entries[0]["MessageBody"].as_str().unwrap()).unwrap();
    message["index"].as_u64().unwrap() as usize
}

#[tokio::test(start_paused = true)]
async fn delayed_preparation_still_reports_confirmed_custody_before_the_request_times_out() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let gate = Arc::new(Gate {
        entered: Mutex::new(Some(entered_tx)),
        release: Mutex::new(Some(release_rx)),
    });
    let (calls_tx, mut calls_rx) = mpsc::unbounded_channel();
    let state = state(calls_tx, Some(gate));
    let body = r#"[{"sourceListingId":"same"},{"sourceListingId":""},{"sourceListingId":"same"},{"sourceListingId":"same"}]"#;
    let first = tokio::spawn({
        let state = state.clone();
        async move { send(&state, body, Some("kept-key"), TIMEOUT).await }
    });
    entered_rx.await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await; // time spent preparing the request
    assert!(
        calls_rx.try_recv().is_err(),
        "no SQS send before authorization"
    );
    release_tx.send(()).unwrap();
    let first_call = calls_rx.recv().await.unwrap();
    let second_call = calls_rx.recv().await.unwrap();
    assert_eq!(wire_index(&first_call), 0);
    assert_eq!(wire_index(&second_call), 2);
    assert_ne!(
        first_call["Entries"][0]["MessageBody"],
        second_call["Entries"][0]["MessageBody"]
    );
    assert_eq!(
        first_call["Entries"][0]["MessageGroupId"],
        second_call["Entries"][0]["MessageGroupId"]
    );
    assert!(!first.is_finished());

    // Same warm publisher, different request. A hung previous send cannot own its budget.
    let fresh = send(
        &state,
        r#"[{"sourceListingId":"fresh"}]"#,
        Some("fresh-key"),
        TIMEOUT,
    )
    .await;
    let (status, key, fresh_report) = report(fresh).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(key, "fresh-key");
    assert_eq!(fresh_report["acceptedCount"], 1);
    assert_eq!(wire_index(&calls_rx.recv().await.unwrap()), 0);

    // The request times out at 4s; publication stops at 3.7s, leaving response headroom.
    tokio::time::advance(Duration::from_millis(1750)).await;
    let (status, key, body) = report(first.await.unwrap()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(key, "kept-key");
    assert!(body["submissionId"].as_str().unwrap().starts_with("plis1_"));
    assert_eq!(body["acceptedCount"], 1);
    let confirmed_message: Value =
        serde_json::from_str(first_call["Entries"][0]["MessageBody"].as_str().unwrap()).unwrap();
    assert_eq!(confirmed_message["listingSourceId"], SOURCE_ID);
    assert_eq!(confirmed_message["submissionId"], body["submissionId"]);
    assert_eq!(confirmed_message["inputCount"], 4);
    assert_eq!(
        body["failures"],
        json!([
            {"index": 1, "error": "BAD_BODY_VALUE", "retryable": false},
            {"index": 2, "sourceListingId": "same", "error": "ENQUEUE_UNCONFIRMED", "retryable": true},
            {"index": 3, "sourceListingId": "same", "error": "ENQUEUE_BLOCKED", "retryable": true}
        ])
    );
    assert!(calls_rx.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn zero_acceptance_and_expired_request_return_reports_without_poisoning_next_call() {
    let (calls_tx, mut calls_rx) = mpsc::unbounded_channel();
    let state = state(calls_tx, None);
    let stalled = tokio::spawn({
        let state = state.clone();
        async move { send(&state, r#"[{"sourceListingId":"stall"}]"#, None, TIMEOUT).await }
    });
    assert_eq!(wire_index(&calls_rx.recv().await.unwrap()), 0);
    tokio::time::advance(Duration::from_millis(3750)).await;
    let (status, generated_key, body) = report(stalled.await.unwrap()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!generated_key.is_empty());
    assert_eq!(body["acceptedCount"], 0);
    assert_eq!(
        body["failures"],
        json!([{
            "index": 0, "sourceListingId": "stall", "error": "ENQUEUE_UNCONFIRMED", "retryable": true
        }])
    );

    let expired_body = r#"[{"sourceListingId":"fresh"}]"#;
    // A request whose budget ends before publication could start.
    let (status, key, expired) = report(
        send(
            &state,
            expired_body,
            Some("retry-key"),
            Duration::from_millis(100),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(key, "retry-key");
    assert_eq!(expired["acceptedCount"], 0);
    assert_eq!(
        expired["failures"],
        json!([{
            "index": 0, "sourceListingId": "fresh", "error": "ENQUEUE_NOT_ATTEMPTED", "retryable": true
        }])
    );
    assert!(
        calls_rx.try_recv().is_err(),
        "expired request must not send"
    );

    let (status, key, retry) =
        report(send(&state, expired_body, Some("retry-key"), TIMEOUT).await).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(key, "retry-key");
    assert_eq!(retry["submissionId"], expired["submissionId"]);
    assert_eq!(retry["acceptedCount"], 1);
    assert_eq!(retry["failures"], json!([]));
    assert_eq!(wire_index(&calls_rx.recv().await.unwrap()), 0);
    assert!(calls_rx.try_recv().is_err());
}
