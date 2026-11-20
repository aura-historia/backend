//! Deadline regression across the HTTP router and its request timeout, the shared HTTP
//! middleware, partner admission, and the real queue publisher. Only the enqueue statement is
//! fake.
use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app_with_request_timeout,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, AsyncPartnerProductListingsState},
};
use axum::{Router, body::to_bytes, http::StatusCode};
use product_listing_ingestion_queue::{
    AdmissionError, IngestionAdmission, IngestionJob, QueueProductListingIngestionPublisher,
    ScopedProductListingIngestionPublisher,
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

/// One enqueue the publisher started: its FIFO group and decoded wire message.
#[derive(Debug)]
struct Sent {
    group: String,
    message: Value,
}

// Every command goes through the publisher's encoding, FIFO keys and deadline handling. Only the
// enqueue statement is fake: index 2 and listing `stall` never return.
struct Admission {
    sent: mpsc::UnboundedSender<Sent>,
}

#[async_trait::async_trait]
impl IngestionAdmission for Admission {
    async fn enqueue(&self, job: &IngestionJob) -> Result<(), AdmissionError> {
        let message: Value = serde_json::from_str(job.body()).unwrap();
        let stalled =
            message["index"] == 2 || message["payload"]["command"]["sourceListingId"] == "stall";
        self.sent
            .send(Sent {
                group: job.group_key().as_str().to_owned(),
                message,
            })
            .expect("test must observe all enqueues");
        if stalled {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
}

/// One warm composition: the same state and publisher serve every request.
fn state(sent: mpsc::UnboundedSender<Sent>, gate: Option<Arc<Gate>>) -> AppState {
    let publisher = ScopedProductListingIngestionPublisher::new(
        QueueProductListingIngestionPublisher::new(Admission { sent }),
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

fn wire_index(sent: &Sent) -> usize {
    sent.message["index"].as_u64().unwrap() as usize
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
        "no enqueue before authorization"
    );
    release_tx.send(()).unwrap();
    let first_call = calls_rx.recv().await.unwrap();
    let second_call = calls_rx.recv().await.unwrap();
    assert_eq!(wire_index(&first_call), 0);
    assert_eq!(wire_index(&second_call), 2);
    assert_ne!(first_call.message, second_call.message);
    assert_eq!(first_call.group, second_call.group);
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
    let confirmed_message = &first_call.message;
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
