use crate::auth::request_metadata;
use crate::error::{
    ApiError, STRIPE_WEBHOOK_INTERNAL_ERROR, STRIPE_WEBHOOK_INVALID_BODY,
    STRIPE_WEBHOOK_PAYLOAD_TOO_LARGE, STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE,
    STRIPE_WEBHOOK_UNAUTHORIZED,
};
use crate::state::StripeWebhooksState;
use application::operation_context::{OperationContext, Principal};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use std::time::Duration;
use user_service::ports::stripe_subscription_sync::{
    StripeSubscriptionSyncError, StripeWebhookEvent, StripeWebhookVerificationError,
};
use user_service::use_cases::ApplyStripeSubscriptionError;

const SIGNATURE_HEADER: HeaderName = HeaderName::from_static("stripe-signature");
/// Covers the bounded Stripe provider read plus the two short reconciliation transactions.
pub(crate) const PROCESSING_BUDGET: Duration = Duration::from_secs(20);
const _: () =
    assert!(PROCESSING_BUDGET.as_millis() < crate::transport::NATIVE_REQUEST_TIMEOUT.as_millis());

/// Authenticates a Stripe delivery by its signature over the exact body, then runs the
/// subscription reconciliation synchronously. Stripe retries every non-2xx answer.
pub async fn post_stripe(
    State(state): State<StripeWebhooksState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > crate::transport::MAX_STRIPE_WEBHOOK_BODY_BYTES {
        return payload_too_large().into_response();
    }
    let mut signatures = headers.get_all(SIGNATURE_HEADER).iter();
    let (Some(signature), None) = (signatures.next(), signatures.next()) else {
        return unauthorized().into_response();
    };
    let arrived_at = time::OffsetDateTime::now_utc().unix_timestamp();
    let command = match state
        .verifier
        .verify(signature.as_bytes(), &body, arrived_at)
    {
        Ok(StripeWebhookEvent::Subscription(command)) => command,
        Ok(StripeWebhookEvent::Ignored) => return StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            log_verification_failure(error);
            return verification_error(error).into_response();
        }
    };
    let event_id = command.event_id.clone();
    let metadata = request_metadata(&headers);
    let context = OperationContext {
        principal: Principal::System,
        request_id: metadata.request_id,
        correlation_id: metadata.correlation_id,
    };
    match tokio::time::timeout(
        PROCESSING_BUDGET,
        state.apply_subscription.execute(&context, command),
    )
    .await
    {
        Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => {
            let response = reconciliation_error(&error).into_response();
            log_reconciliation_failure(&event_id, &error, response.status());
            response
        }
        Err(_) => {
            tracing::warn!(
                event = "stripe_webhook.reconciliation_failed",
                stripe_event_id = %event_id,
                status = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "Stripe subscription reconciliation exceeded its processing budget"
            );
            temporarily_unavailable().into_response()
        }
    }
}

/// Unauthenticated requests are not logged: anyone can send them. An authentic event that
/// is rejected is, because Stripe keeps retrying it and only an operator can resolve it.
fn log_verification_failure(error: StripeWebhookVerificationError) {
    match error {
        StripeWebhookVerificationError::InvalidSignature => {}
        StripeWebhookVerificationError::MalformedEvent => tracing::warn!(
            event = "stripe_webhook.rejected",
            reason = "malformed_event",
            "Authentic Stripe event is malformed or rendered with another API version"
        ),
        StripeWebhookVerificationError::CryptographicFailure => tracing::error!(
            event = "stripe_webhook.reconciliation_failed",
            status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "Stripe webhook signature could not be computed"
        ),
    }
}

fn log_reconciliation_failure(
    event_id: &str,
    error: &ApplyStripeSubscriptionError,
    status: StatusCode,
) {
    let causes = application::error::error_chain(error)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ");
    if status.is_server_error() {
        tracing::warn!(
            event = "stripe_webhook.reconciliation_failed",
            stripe_event_id = %event_id,
            status = status.as_u16(),
            error = %causes,
            "Stripe subscription reconciliation failed; Stripe retries the delivery"
        );
    } else {
        tracing::warn!(
            event = "stripe_webhook.rejected",
            stripe_event_id = %event_id,
            status = status.as_u16(),
            error = %causes,
            "Authentic Stripe event was rejected; Stripe retries the delivery"
        );
    }
}

fn verification_error(error: StripeWebhookVerificationError) -> ApiError {
    match error {
        StripeWebhookVerificationError::InvalidSignature => unauthorized(),
        StripeWebhookVerificationError::MalformedEvent => invalid_body(),
        StripeWebhookVerificationError::CryptographicFailure => internal_error(),
    }
}

/// Requests that can never succeed answer `400`; everything else is retried by Stripe:
/// transient failures answer `503`, broken invariants needing an operator answer `500`.
fn reconciliation_error(error: &ApplyStripeSubscriptionError) -> ApiError {
    match error {
        ApplyStripeSubscriptionError::Synchronization(
            StripeSubscriptionSyncError::InvalidEvent
            | StripeSubscriptionSyncError::ReceiptConflict,
        ) => invalid_body(),
        ApplyStripeSubscriptionError::Synchronization(
            StripeSubscriptionSyncError::Unavailable { .. }
            | StripeSubscriptionSyncError::ConcurrentReconciliation,
        )
        | ApplyStripeSubscriptionError::UserNotFound
        | ApplyStripeSubscriptionError::ConcurrencyConflict
        | ApplyStripeSubscriptionError::TemporarilyUnavailable { .. }
        | ApplyStripeSubscriptionError::TierEntitlementsLockFailed { .. }
        | ApplyStripeSubscriptionError::BeginTransactionFailed(_)
        | ApplyStripeSubscriptionError::CommitTransactionFailed(_) => temporarily_unavailable(),
        ApplyStripeSubscriptionError::Synchronization(
            StripeSubscriptionSyncError::InvalidState { .. },
        )
        | ApplyStripeSubscriptionError::CustomerOwnershipConflict
        | ApplyStripeSubscriptionError::StripeCustomerConflict { .. }
        | ApplyStripeSubscriptionError::TierEntitlementsReconciliationFailed { .. }
        | ApplyStripeSubscriptionError::InvalidPersistedState { .. }
        | ApplyStripeSubscriptionError::Internal { .. }
        | ApplyStripeSubscriptionError::AuthenticatedActorRequired
        | ApplyStripeSubscriptionError::Forbidden => internal_error(),
    }
}

fn unauthorized() -> ApiError {
    ApiError::unauthorized(STRIPE_WEBHOOK_UNAUTHORIZED)
        .with_detail("Webhook authentication failed.")
}

fn invalid_body() -> ApiError {
    ApiError::bad_request(STRIPE_WEBHOOK_INVALID_BODY)
        .with_detail("The webhook event is invalid or conflicts with a processed event.")
}

fn temporarily_unavailable() -> ApiError {
    ApiError::service_unavailable(STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE)
        .with_detail("Webhook processing must be retried.")
}

fn internal_error() -> ApiError {
    ApiError::internal_server_error(STRIPE_WEBHOOK_INTERNAL_ERROR)
        .with_detail("Webhook processing failed and must be retried.")
}

pub(crate) fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "Payload Too Large",
        STRIPE_WEBHOOK_PAYLOAD_TOO_LARGE,
    )
    .with_detail("The webhook body exceeds the supported limit.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use application::error::static_error;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use billing_stripe::{
        STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED, STRIPE_WEBHOOK_API_VERSION,
        StripeSubscriptionWebhookVerifier,
    };
    use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use user_service::ports::stripe_subscription_sync::StripeSubscriptionEvent;
    use user_service::use_cases::{ApplyStripeSubscriptionResult, ApplyStripeSubscriptionUseCase};

    const SECRET: &str = "whsec_test_current";
    const PATH: &str = "/v1/webhooks/stripe";

    type Outcome = fn() -> Result<ApplyStripeSubscriptionResult, ApplyStripeSubscriptionError>;

    #[derive(Clone)]
    struct RecordingUseCase {
        commands: Arc<Mutex<Vec<(Principal, StripeSubscriptionEvent)>>>,
        outcome: Outcome,
        delay: Duration,
    }

    impl RecordingUseCase {
        fn new(outcome: Outcome) -> Self {
            Self {
                commands: Arc::default(),
                outcome,
                delay: Duration::ZERO,
            }
        }

        fn calls(&self) -> usize {
            self.commands
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len()
        }
    }

    #[async_trait::async_trait]
    impl ApplyStripeSubscriptionUseCase for RecordingUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: StripeSubscriptionEvent,
        ) -> Result<ApplyStripeSubscriptionResult, ApplyStripeSubscriptionError> {
            self.commands
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((context.principal.clone(), command));
            tokio::time::sleep(self.delay).await;
            (self.outcome)()
        }
    }

    fn app_for(use_case: RecordingUseCase) -> axum::Router {
        crate::app(
            AppState::new().with_stripe_webhooks(StripeWebhooksState::new(
                Arc::new(StripeSubscriptionWebhookVerifier::new(SECRET).expect("secret")),
                Arc::new(use_case),
            )),
        )
    }

    fn event_body(kind: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": "evt_1",
            "object": "event",
            "api_version": STRIPE_WEBHOOK_API_VERSION,
            "type": kind,
            "data": {"object": {"id": "sub_1", "object": "subscription", "customer": "cus_1"}},
        }))
        .expect("event JSON")
    }

    fn signature(body: &[u8]) -> String {
        let timestamp = time::OffsetDateTime::now_utc().unix_timestamp();
        let key = PKey::hmac(SECRET.as_bytes()).expect("HMAC key");
        let mut signer = Signer::new(MessageDigest::sha256(), &key).expect("HMAC signer");
        signer
            .update(format!("{timestamp}.").as_bytes())
            .expect("timestamp");
        signer.update(body).expect("body");
        let digest: String = signer
            .sign_to_vec()
            .expect("HMAC")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("t={timestamp},v1={digest}")
    }

    fn signed_request(body: Vec<u8>) -> Request<Body> {
        let signature = signature(&body);
        Request::builder()
            .method("POST")
            .uri(PATH)
            .header("stripe-signature", signature)
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
    async fn should_reconcile_a_signed_subscription_event_as_the_system_without_a_bearer() {
        let use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));

        let response = app_for(use_case.clone())
            .oneshot(signed_request(event_body(
                STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            )))
            .await
            .expect("route response");

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        let commands = use_case.commands.lock().expect("commands");
        assert_eq!(1, commands.len());
        assert_eq!(Principal::System, commands[0].0);
        assert_eq!("evt_1", commands[0].1.event_id);
        assert_eq!("sub_1", commands[0].1.subscription_id);
        assert_eq!("cus_1", commands[0].1.customer_id.as_ref());
    }

    #[tokio::test]
    async fn should_acknowledge_other_event_types_without_reconciling() {
        let use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));

        let response = app_for(use_case.clone())
            .oneshot(signed_request(event_body("invoice.paid")))
            .await
            .expect("route response");

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(0, use_case.calls());
    }

    #[tokio::test]
    async fn should_answer_401_without_reconciling_when_the_signature_is_missing_invalid_or_repeated()
     {
        let use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));
        let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED);
        let mut missing = signed_request(body.clone());
        missing.headers_mut().remove("stripe-signature");
        let mut invalid = signed_request(body.clone());
        invalid.headers_mut().insert(
            "stripe-signature",
            "t=1,v1=00".parse().expect("header value"),
        );
        let mut repeated = signed_request(body.clone());
        let signature = repeated.headers()["stripe-signature"].clone();
        repeated.headers_mut().append("stripe-signature", signature);

        for request in [missing, invalid, repeated] {
            let response = app_for(use_case.clone())
                .oneshot(request)
                .await
                .expect("route response");

            assert_eq!(StatusCode::UNAUTHORIZED, response.status());
            assert_eq!("STRIPE_WEBHOOK_UNAUTHORIZED", problem_code(response).await);
        }
        assert_eq!(0, use_case.calls());
    }

    #[tokio::test]
    async fn should_answer_400_without_reconciling_an_authentic_malformed_event() {
        let use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));

        let response = app_for(use_case.clone())
            .oneshot(signed_request(b"{\"type\":".to_vec()))
            .await
            .expect("route response");

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!("STRIPE_WEBHOOK_INVALID_BODY", problem_code(response).await);
        assert_eq!(0, use_case.calls());
    }

    #[tokio::test]
    async fn should_map_each_reconciliation_outcome_to_its_status() {
        let cases: [(Outcome, StatusCode, Option<&str>); 11] = [
            (
                || Ok(ApplyStripeSubscriptionResult::Applied),
                StatusCode::NO_CONTENT,
                None,
            ),
            (
                || Ok(ApplyStripeSubscriptionResult::AlreadyApplied),
                StatusCode::NO_CONTENT,
                None,
            ),
            (
                || Err(StripeSubscriptionSyncError::InvalidEvent.into()),
                StatusCode::BAD_REQUEST,
                Some("STRIPE_WEBHOOK_INVALID_BODY"),
            ),
            (
                || Err(StripeSubscriptionSyncError::ReceiptConflict.into()),
                StatusCode::BAD_REQUEST,
                Some("STRIPE_WEBHOOK_INVALID_BODY"),
            ),
            (
                || {
                    Err(StripeSubscriptionSyncError::Unavailable {
                        source: static_error("provider outage"),
                    }
                    .into())
                },
                StatusCode::SERVICE_UNAVAILABLE,
                Some("STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE"),
            ),
            (
                || Err(StripeSubscriptionSyncError::ConcurrentReconciliation.into()),
                StatusCode::SERVICE_UNAVAILABLE,
                Some("STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE"),
            ),
            (
                || {
                    Err(ApplyStripeSubscriptionError::InvalidPersistedState {
                        source: static_error("invalid tier"),
                    })
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("STRIPE_WEBHOOK_INTERNAL_ERROR"),
            ),
            (
                || Err(ApplyStripeSubscriptionError::UserNotFound),
                StatusCode::SERVICE_UNAVAILABLE,
                Some("STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE"),
            ),
            (
                || {
                    Err(ApplyStripeSubscriptionError::CommitTransactionFailed(
                        static_error("uncertain commit"),
                    ))
                },
                StatusCode::SERVICE_UNAVAILABLE,
                Some("STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE"),
            ),
            (
                || Err(ApplyStripeSubscriptionError::CustomerOwnershipConflict),
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("STRIPE_WEBHOOK_INTERNAL_ERROR"),
            ),
            (
                || {
                    Err(StripeSubscriptionSyncError::InvalidState {
                        source: static_error("unmapped product"),
                    }
                    .into())
                },
                StatusCode::INTERNAL_SERVER_ERROR,
                Some("STRIPE_WEBHOOK_INTERNAL_ERROR"),
            ),
        ];
        for (outcome, status, code) in cases {
            let response = app_for(RecordingUseCase::new(outcome))
                .oneshot(signed_request(event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                )))
                .await
                .expect("route response");

            assert_eq!(status, response.status());
            if let Some(code) = code {
                assert_eq!(code, problem_code(response).await);
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_answer_503_when_the_processing_budget_expires() {
        let mut use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));
        use_case.delay = PROCESSING_BUDGET + Duration::from_secs(1);

        let response = app_for(use_case)
            .oneshot(signed_request(event_body(
                STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
            )))
            .await
            .expect("route response");

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        assert_eq!(
            "STRIPE_WEBHOOK_TEMPORARILY_UNAVAILABLE",
            problem_code(response).await
        );
    }

    #[tokio::test]
    async fn should_enforce_the_route_body_limit_with_a_problem_response() {
        let use_case = RecordingUseCase::new(|| Ok(ApplyStripeSubscriptionResult::Applied));

        let response = app_for(use_case.clone())
            .oneshot(signed_request(vec![
                b'x';
                crate::transport::MAX_STRIPE_WEBHOOK_BODY_BYTES
                    + 1
            ]))
            .await
            .expect("route response");

        assert_eq!(StatusCode::PAYLOAD_TOO_LARGE, response.status());
        assert_eq!(
            "STRIPE_WEBHOOK_PAYLOAD_TOO_LARGE",
            problem_code(response).await
        );
        assert_eq!(0, use_case.calls());
    }
}
