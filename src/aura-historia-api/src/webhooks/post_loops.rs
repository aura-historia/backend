use crate::error::{
    ApiError, LOOPS_WEBHOOK_CONFLICT, LOOPS_WEBHOOK_INTERNAL_ERROR, LOOPS_WEBHOOK_INVALID_BODY,
    LOOPS_WEBHOOK_PAYLOAD_TOO_LARGE, LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE,
    LOOPS_WEBHOOK_UNAUTHORIZED,
};
use crate::state::LoopsWebhooksState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use std::time::Duration;
use user_service::ports::{
    NewsletterWebhookHeader, NewsletterWebhookVerificationError,
    NewsletterWebhookVerificationRequest,
};
use user_service::use_cases::{
    ApplyLoopsPreferenceEventCommand, ApplyLoopsPreferenceEventError,
    ApplyLoopsPreferenceEventOutcome,
};

const DELIVERY_ID_HEADER: HeaderName = HeaderName::from_static("webhook-id");
const TIMESTAMP_HEADER: HeaderName = HeaderName::from_static("webhook-timestamp");
const SIGNATURE_HEADER: HeaderName = HeaderName::from_static("webhook-signature");
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADER_COUNT: usize = 16;
const MAX_HEADER_VALUE_BYTES: usize = 8 * 1024;
const PROCESSING_BUDGET: Duration = Duration::from_secs(10);

pub async fn post_loops(
    State(state): State<LoopsWebhooksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > crate::transport::MAX_LOOPS_WEBHOOK_BODY_BYTES {
        return payload_too_large().into_response();
    }

    let selected_headers = match selected_headers(&headers) {
        Ok(headers) => headers,
        Err(()) => return unauthorized().into_response(),
    };
    let deadline = tokio::time::Instant::now() + PROCESSING_BUDGET;
    let request = NewsletterWebhookVerificationRequest {
        headers: selected_headers,
        raw_body: body.to_vec(),
        signing_secret: state.signing_secret.to_string(),
        arrived_at_unix_seconds: time::OffsetDateTime::now_utc().unix_timestamp(),
    };
    let verification = match state.verifier.verify(request) {
        Ok(verification) => verification,
        Err(error) => return verification_error(error).into_response(),
    };

    match tokio::time::timeout_at(
        deadline,
        state
            .apply_preference_event
            .execute(ApplyLoopsPreferenceEventCommand { verification }),
    )
    .await
    {
        Ok(Ok(
            ApplyLoopsPreferenceEventOutcome::CommittedApplication(_)
            | ApplyLoopsPreferenceEventOutcome::Duplicate
            | ApplyLoopsPreferenceEventOutcome::Ignored(_),
        )) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(ApplyLoopsPreferenceEventOutcome::Conflict)) => {
            ApiError::conflict(LOOPS_WEBHOOK_CONFLICT)
                .with_detail(
                    "The authenticated delivery conflicts with a previously received event.",
                )
                .into_response()
        }
        Ok(Err(ApplyLoopsPreferenceEventError::InvalidEventTime)) => invalid_body().into_response(),
        Ok(Err(ApplyLoopsPreferenceEventError::Retryable)) | Err(_) => {
            ApiError::service_unavailable(LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE)
                .with_detail("Webhook processing must be retried.")
                .into_response()
        }
    }
}

fn selected_headers(headers: &HeaderMap) -> Result<Vec<NewsletterWebhookHeader>, ()> {
    let mut selected = Vec::with_capacity(3);
    let mut total_bytes = 0usize;
    for name in [DELIVERY_ID_HEADER, TIMESTAMP_HEADER, SIGNATURE_HEADER] {
        for value in headers.get_all(&name).iter() {
            if value.len() > MAX_HEADER_VALUE_BYTES || selected.len() == MAX_HEADER_COUNT {
                return Err(());
            }
            total_bytes = total_bytes
                .checked_add(name.as_str().len())
                .and_then(|size| size.checked_add(value.len()))
                .ok_or(())?;
            if total_bytes > MAX_HEADER_BYTES {
                return Err(());
            }
            selected.push(NewsletterWebhookHeader::new(
                name.as_str().as_bytes().to_vec(),
                value.as_bytes().to_vec(),
            ));
        }
    }
    Ok(selected)
}

fn verification_error(error: NewsletterWebhookVerificationError) -> ApiError {
    match error {
        NewsletterWebhookVerificationError::BodyTooLarge => payload_too_large(),
        NewsletterWebhookVerificationError::MalformedPayload
        | NewsletterWebhookVerificationError::UnsupportedSchema
        | NewsletterWebhookVerificationError::InvalidEvent => invalid_body(),
        NewsletterWebhookVerificationError::MissingSigningSecret
        | NewsletterWebhookVerificationError::InvalidSigningSecret
        | NewsletterWebhookVerificationError::CryptographicFailure => {
            ApiError::internal_server_error(LOOPS_WEBHOOK_INTERNAL_ERROR)
                .with_detail("Webhook processing is not configured correctly.")
        }
        NewsletterWebhookVerificationError::HeadersTooLarge
        | NewsletterWebhookVerificationError::MissingRequiredHeader
        | NewsletterWebhookVerificationError::AmbiguousHeader
        | NewsletterWebhookVerificationError::InvalidHeaderEncoding
        | NewsletterWebhookVerificationError::InvalidSignatureHeader
        | NewsletterWebhookVerificationError::InvalidTimestamp
        | NewsletterWebhookVerificationError::InvalidSignature => unauthorized(),
    }
}

fn unauthorized() -> ApiError {
    ApiError::unauthorized(LOOPS_WEBHOOK_UNAUTHORIZED).with_detail("Webhook authentication failed.")
}

fn invalid_body() -> ApiError {
    ApiError::bad_request(LOOPS_WEBHOOK_INVALID_BODY)
        .with_detail("The webhook body or selected event is invalid.")
}

fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "Payload Too Large",
        LOOPS_WEBHOOK_PAYLOAD_TOO_LARGE,
    )
    .with_detail("The webhook body exceeds the supported limit.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, LoopsWebhooksState};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use base64::Engine as _;
    use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
    use sha2::{Digest, Sha256};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;
    use user_loops::LoopsNewsletterWebhookVerifier;
    use user_service::ports::LoopsWebhookReceiptDisposition;
    use user_service::use_cases::ApplyLoopsPreferenceEventUseCase;

    const SECRET: &str = "whsec_bG9vcHMta2V5LWN1cnJlbnQ=";
    const SIGNING_KEY: &[u8] = b"loops-key-current";

    #[derive(Clone)]
    struct RecordingUseCase {
        calls: Arc<AtomicUsize>,
        hashes: Arc<Mutex<Vec<[u8; 32]>>>,
        result: Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError>,
        delay: Duration,
    }

    impl RecordingUseCase {
        fn new(
            result: Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError>,
        ) -> Self {
            Self {
                calls: Arc::new(AtomicUsize::new(0)),
                hashes: Arc::new(Mutex::new(Vec::new())),
                result,
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }
    }

    #[async_trait::async_trait]
    impl ApplyLoopsPreferenceEventUseCase for RecordingUseCase {
        async fn execute(
            &self,
            command: ApplyLoopsPreferenceEventCommand,
        ) -> Result<ApplyLoopsPreferenceEventOutcome, ApplyLoopsPreferenceEventError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.hashes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(*command.verification.raw_body_sha256.as_bytes());
            tokio::time::sleep(self.delay).await;
            self.result
        }
    }

    fn app_for(use_case: RecordingUseCase) -> axum::Router {
        crate::app(AppState::new().with_loops_webhooks(LoopsWebhooksState::new(
            SECRET,
            Arc::new(LoopsNewsletterWebhookVerifier),
            Arc::new(use_case),
        )))
    }

    fn signed_fixture() -> (Vec<u8>, String, String, String) {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let body = format!(
            " {{\n  \"webhookSchemaVersion\" : \"1.0.0\", \"eventName\": \"testing.testEvent\", \"eventTime\": {}, \"unused\": \"élève\" \n}}",
            timestamp.parse::<i64>().unwrap_or_default()
        )
        .into_bytes();
        let delivery_id = "msg_fixture_1948".to_owned();
        let signature = sign(&delivery_id, &timestamp, &body);
        (body, delivery_id, timestamp, signature)
    }

    fn sign(delivery_id: &str, timestamp: &str, body: &[u8]) -> String {
        let key = PKey::hmac(SIGNING_KEY).expect("HMAC key");
        let mut signer = Signer::new(MessageDigest::sha256(), &key).expect("HMAC signer");
        signer.update(delivery_id.as_bytes()).expect("delivery id");
        signer.update(b".").expect("separator");
        signer.update(timestamp.as_bytes()).expect("timestamp");
        signer.update(b".").expect("separator");
        signer.update(body).expect("body bytes");
        format!(
            "v1,{}",
            base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().expect("HMAC"))
        )
    }

    fn request(
        body: Vec<u8>,
        delivery_id: &str,
        timestamp: &str,
        signature: &str,
    ) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/v1/webhooks/loops")
            .header("webhook-id", delivery_id)
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(Body::from(body))
            .expect("valid request")
    }

    async fn problem_code(response: Response) -> String {
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem body");
        serde_json::from_slice::<serde_json::Value>(&body).expect("problem JSON")["error"]
            .as_str()
            .expect("error code")
            .to_owned()
    }

    #[tokio::test]
    async fn accepts_a_valid_signed_event_without_a_bearer_and_preserves_exact_bytes() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Ignored(
            LoopsWebhookReceiptDisposition::IgnoredUnsupportedEvent,
        )));
        let calls = Arc::clone(&use_case.calls);
        let hashes = Arc::clone(&use_case.hashes);
        let (body, delivery_id, timestamp, signature) = signed_fixture();
        let expected_digest: [u8; 32] = Sha256::digest(&body).into();

        let response = app_for(use_case)
            .oneshot(request(body, &delivery_id, &timestamp, &signature))
            .await
            .expect("route response");

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!(
            0,
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .len()
        );
        assert_eq!(1, calls.load(Ordering::SeqCst));
        assert_eq!(vec![expected_digest], hashes.lock().unwrap().clone());
    }

    #[tokio::test]
    async fn invalid_or_ambiguous_provider_proof_returns_generic_401_without_application() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
        let calls = Arc::clone(&use_case.calls);
        let (body, delivery_id, timestamp, _) = signed_fixture();
        let missing = app_for(use_case.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/loops")
                    .body(Body::empty())
                    .expect("request without proof"),
            )
            .await
            .expect("route response");
        assert_eq!(StatusCode::UNAUTHORIZED, missing.status());
        assert_eq!(
            "private, no-store",
            missing.headers()[header::CACHE_CONTROL]
        );
        assert_eq!("LOOPS_WEBHOOK_UNAUTHORIZED", problem_code(missing).await);

        let invalid = app_for(use_case.clone())
            .oneshot(request(body.clone(), &delivery_id, &timestamp, "v1,AAAA"))
            .await
            .expect("route response");
        assert_eq!(StatusCode::UNAUTHORIZED, invalid.status());
        assert_eq!("LOOPS_WEBHOOK_UNAUTHORIZED", problem_code(invalid).await);

        let mut ambiguous = request(
            body,
            &delivery_id,
            &timestamp,
            &sign(&delivery_id, &timestamp, b"not-the-body"),
        );
        ambiguous
            .headers_mut()
            .append("webhook-id", delivery_id.parse().expect("header value"));
        let ambiguous = app_for(use_case)
            .oneshot(ambiguous)
            .await
            .expect("route response");
        assert_eq!(StatusCode::UNAUTHORIZED, ambiguous.status());
        assert_eq!("LOOPS_WEBHOOK_UNAUTHORIZED", problem_code(ambiguous).await);
        assert_eq!(0, calls.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn valid_proof_for_malformed_body_returns_safe_400_without_application() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
        let calls = Arc::clone(&use_case.calls);
        let delivery_id = "msg_malformed_fixture";
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
        let body = b"{\"webhookSchemaVersion\":".to_vec();
        let signature = sign(delivery_id, &timestamp, &body);
        let response = app_for(use_case)
            .oneshot(request(body, delivery_id, &timestamp, &signature))
            .await
            .expect("route response");

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("LOOPS_WEBHOOK_INVALID_BODY", problem_code(response).await);
        assert_eq!(0, calls.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn maps_service_conflict_retryable_failure_and_invalid_event_time() {
        let (body, delivery_id, timestamp, signature) = signed_fixture();
        for (result, expected_status, expected_code) in [
            (
                Ok(ApplyLoopsPreferenceEventOutcome::Conflict),
                StatusCode::CONFLICT,
                "LOOPS_WEBHOOK_CONFLICT",
            ),
            (
                Err(ApplyLoopsPreferenceEventError::Retryable),
                StatusCode::SERVICE_UNAVAILABLE,
                "LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE",
            ),
            (
                Err(ApplyLoopsPreferenceEventError::InvalidEventTime),
                StatusCode::BAD_REQUEST,
                "LOOPS_WEBHOOK_INVALID_BODY",
            ),
        ] {
            let response = app_for(RecordingUseCase::new(result))
                .oneshot(request(body.clone(), &delivery_id, &timestamp, &signature))
                .await
                .expect("route response");
            assert_eq!(expected_status, response.status());
            assert_eq!(expected_code, problem_code(response).await);
        }
    }

    #[tokio::test]
    async fn enforces_the_route_body_limit_with_a_problem_response() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
        let calls = Arc::clone(&use_case.calls);
        let response = app_for(use_case)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/loops")
                    .body(Body::from(vec![
                        b'x';
                        crate::transport::MAX_LOOPS_WEBHOOK_BODY_BYTES
                            + 1
                    ]))
                    .expect("oversized request"),
            )
            .await
            .expect("route response");

        assert_eq!(StatusCode::PAYLOAD_TOO_LARGE, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!(
            "LOOPS_WEBHOOK_PAYLOAD_TOO_LARGE",
            problem_code(response).await
        );
        assert_eq!(0, calls.load(Ordering::SeqCst));
    }

    #[tokio::test(start_paused = true)]
    async fn returns_retryable_failure_when_the_processing_budget_expires() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Duplicate))
            .with_delay(Duration::from_secs(11));
        let (body, delivery_id, timestamp, signature) = signed_fixture();

        let response = app_for(use_case)
            .oneshot(request(body, &delivery_id, &timestamp, &signature))
            .await
            .expect("route response");

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        assert_eq!(
            "LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE",
            problem_code(response).await
        );
    }

    #[tokio::test]
    async fn accepts_http_api_v2_base64_body_with_its_original_signature() {
        let use_case = RecordingUseCase::new(Ok(ApplyLoopsPreferenceEventOutcome::Duplicate));
        let hashes = Arc::clone(&use_case.hashes);
        let (body, delivery_id, timestamp, signature) = signed_fixture();
        let expected_digest: [u8; 32] = Sha256::digest(&body).into();
        let mut event: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/http_api_v2_health.json"))
                .expect("HTTP API v2 fixture");
        event["routeKey"] = "POST /api/v1/webhooks/loops".into();
        event["rawPath"] = "/api/v1/webhooks/loops".into();
        event["requestContext"]["http"]["method"] = "POST".into();
        event["requestContext"]["http"]["path"] = "/api/v1/webhooks/loops".into();
        event["headers"] = serde_json::json!({
            "host": "api.example.test",
            "webhook-id": delivery_id,
            "webhook-timestamp": timestamp,
            "webhook-signature": signature,
        });
        event["body"] = base64::engine::general_purpose::STANDARD
            .encode(&body)
            .into();
        event["isBase64Encoded"] = true.into();
        let request = lambda_http::request::from_str(&event.to_string()).expect("Lambda request");

        let response = crate::lambda::handle_http_api_v2_request(app_for(use_case), request)
            .await
            .expect("Lambda HTTP adapter response");

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(vec![expected_digest], hashes.lock().unwrap().clone());
    }
}
