//! Registers the user of a Cognito post-confirmation event that the trigger's forwarder signed.
//! Cognito fails the confirmation on any non-2xx answer, as it did when the trigger wrote to the
//! database itself; registration is idempotent, so Cognito's own retry and a replay inside the
//! signature window repeat it harmlessly.
use crate::auth::request_metadata;
use crate::error::{
    ApiError, POST_CONFIRMATION_CONFLICT, POST_CONFIRMATION_INTERNAL_ERROR,
    POST_CONFIRMATION_INVALID_EVENT, POST_CONFIRMATION_PAYLOAD_TOO_LARGE,
    POST_CONFIRMATION_TEMPORARILY_UNAVAILABLE, POST_CONFIRMATION_UNAUTHORIZED,
};
use crate::state::CognitoPostConfirmationState;
use application::operation_context::{OperationContext, Principal};
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cognito_post_confirmation::{
    PostConfirmationError, register_confirmed_user,
    signature::{SIGNATURE_HEADER, SignatureError, TIMESTAMP_HEADER},
};
use std::time::Duration;
use user_service::use_cases::RegisterCognitoUserError;

/// A Cognito post-confirmation event is a few kilobytes.
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024;
/// The forwarder gives up after 4.2 seconds; answer before it does.
const PROCESSING_BUDGET: Duration = Duration::from_millis(3_500);

pub async fn post_cognito_post_confirmation(
    State(state): State<CognitoPostConfirmationState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Payload Too Large",
            POST_CONFIRMATION_PAYLOAD_TOO_LARGE,
        )
        .into_response();
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if let Err(error) = state.verifier.verify(
        single_header(&headers, TIMESTAMP_HEADER),
        single_header(&headers, SIGNATURE_HEADER),
        &body,
        now,
    ) {
        return verification_error(error).into_response();
    }
    let Ok(event) = serde_json::from_slice::<CognitoEventUserPoolsPostConfirmation>(&body) else {
        return invalid_event().into_response();
    };
    let metadata = request_metadata(&headers);
    let context = OperationContext {
        principal: Principal::System,
        request_id: metadata.request_id,
        correlation_id: metadata.correlation_id,
    };
    let registration = tokio::time::timeout(
        PROCESSING_BUDGET,
        register_confirmed_user(
            &event,
            &state.expected_issuer,
            &context,
            state.register.as_ref(),
        ),
    )
    .await;
    match registration {
        Ok(Ok(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(PostConfirmationError::Input(error))) => {
            tracing::warn!(
                event = "cognito_post_confirmation.rejected",
                reason = %error,
                "Cognito post-confirmation event rejected"
            );
            invalid_event().into_response()
        }
        Ok(Err(PostConfirmationError::Registration(error))) => {
            let response = registration_error(&error);
            tracing::warn!(
                event = "cognito_post_confirmation.failed",
                status = response.status().as_u16(),
                error = %error,
                "Cognito user registration failed"
            );
            response
        }
        Err(_) => {
            tracing::warn!(
                event = "cognito_post_confirmation.failed",
                status = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "Cognito user registration exceeded its processing budget"
            );
            temporarily_unavailable().into_response()
        }
    }
}

/// The header's value only when it is present exactly once.
fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => Some(value.as_bytes()),
        _ => None,
    }
}

fn verification_error(error: SignatureError) -> ApiError {
    tracing::warn!(
        event = "cognito_post_confirmation.unauthorized",
        reason = %error,
        "Cognito post-confirmation request failed authentication"
    );
    match error {
        SignatureError::CryptographicFailure => {
            ApiError::internal_server_error(POST_CONFIRMATION_INTERNAL_ERROR)
        }
        SignatureError::MalformedTimestamp
        | SignatureError::StaleTimestamp
        | SignatureError::MalformedSignature
        | SignatureError::InvalidSignature => {
            ApiError::unauthorized(POST_CONFIRMATION_UNAUTHORIZED)
                .with_detail("Request authentication failed.")
        }
    }
}

fn registration_error(error: &RegisterCognitoUserError) -> Response {
    match error {
        RegisterCognitoUserError::IdentityConflict
        | RegisterCognitoUserError::EmailConflict { .. }
        | RegisterCognitoUserError::IdentityPersistenceConflict { .. } => {
            ApiError::conflict(POST_CONFIRMATION_CONFLICT).into_response()
        }
        RegisterCognitoUserError::TemporarilyUnavailable { .. }
        | RegisterCognitoUserError::BeginTransactionFailed(_)
        | RegisterCognitoUserError::CommitTransactionFailed(_)
        | RegisterCognitoUserError::SignupConsentFailed(_) => {
            temporarily_unavailable().into_response()
        }
        RegisterCognitoUserError::Forbidden
        | RegisterCognitoUserError::InvalidPersistedState { .. }
        | RegisterCognitoUserError::Internal { .. } => {
            ApiError::internal_server_error(POST_CONFIRMATION_INTERNAL_ERROR).into_response()
        }
    }
}

fn invalid_event() -> ApiError {
    ApiError::bad_request(POST_CONFIRMATION_INVALID_EVENT)
        .with_detail("The confirmation event is invalid.")
}

fn temporarily_unavailable() -> ApiError {
    ApiError::service_unavailable(POST_CONFIRMATION_TEMPORARILY_UNAVAILABLE)
        .with_detail("Registration must be retried.")
}

#[cfg(test)]
mod tests {
    use crate::state::{AppState, CognitoPostConfirmationState};
    use application::operation_context::{OperationContext, Principal};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use cognito_post_confirmation::signature::{
        SIGNATURE_HEADER, SignatureVerifier, SigningKey, TIMESTAMP_HEADER,
        TIMESTAMP_TOLERANCE_SECONDS, sign,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use user_core::user_id::UserId;
    use user_service::use_cases::{
        RegisterCognitoUserCommand, RegisterCognitoUserError, RegisterCognitoUserResult,
        RegisterCognitoUserUseCase,
    };

    const PATH: &str = "/internal/cognito/post-confirmation";
    const KEY: &str = "post-confirmation-test-signing-key-0123";
    const ISSUER: &str = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_pool";

    type Failure = fn() -> RegisterCognitoUserError;

    #[derive(Default)]
    struct FakeRegistration {
        failure: Option<Failure>,
        calls: Mutex<Vec<(OperationContext, RegisterCognitoUserCommand)>>,
    }

    #[async_trait::async_trait]
    impl RegisterCognitoUserUseCase for FakeRegistration {
        async fn execute(
            &self,
            context: &OperationContext,
            command: RegisterCognitoUserCommand,
        ) -> Result<RegisterCognitoUserResult, RegisterCognitoUserError> {
            self.calls
                .lock()
                .unwrap()
                .push((context.clone(), command.clone()));
            match self.failure {
                Some(failure) => Err(failure()),
                None => Ok(RegisterCognitoUserResult {
                    user_id: UserId::new(),
                    email: command.email,
                }),
            }
        }
    }

    fn router(registration: Arc<FakeRegistration>) -> axum::Router {
        crate::app(AppState::new().with_cognito_post_confirmation(
            CognitoPostConfirmationState::new(
                SignatureVerifier::new(SigningKey::new(KEY).unwrap(), None),
                ISSUER,
                registration,
            ),
        ))
    }

    fn event(user_pool_id: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "eu-central-1",
            "userPoolId": user_pool_id,
            "userName": "subject-1",
            "callerContext": {},
            "request": {
                "userAttributes": { "sub": "subject-1", "email": "ada@example.test" }
            },
            "response": {}
        }))
        .unwrap()
    }

    fn now() -> i64 {
        time::OffsetDateTime::now_utc().unix_timestamp()
    }

    fn signed(body: Vec<u8>, key: &str, timestamp: i64) -> Request<Body> {
        let signature = sign(&SigningKey::new(key).unwrap(), timestamp, &body).unwrap();
        Request::post(PATH)
            .header("content-type", "application/json")
            .header("x-correlation-id", "lambda-request-id")
            .header(TIMESTAMP_HEADER, timestamp.to_string())
            .header(SIGNATURE_HEADER, signature)
            .body(Body::from(body))
            .unwrap()
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
    async fn should_register_the_user_of_a_validly_signed_confirmation() {
        let registration = Arc::new(FakeRegistration::default());

        let (status, _) = send(
            router(Arc::clone(&registration)),
            signed(event("eu-central-1_pool"), KEY, now()),
        )
        .await;

        assert_eq!(StatusCode::NO_CONTENT, status);
        let calls = registration.calls.lock().unwrap();
        assert_eq!(1, calls.len());
        assert!(matches!(calls[0].0.principal, Principal::System));
        assert_eq!("lambda-request-id", calls[0].0.correlation_id.as_str());
        assert_eq!(ISSUER, calls[0].1.identity.issuer.as_str());
        assert_eq!("subject-1", calls[0].1.identity.subject.as_str());
    }

    #[tokio::test]
    async fn should_refuse_a_replayed_request_whose_timestamp_has_expired() {
        let registration = Arc::new(FakeRegistration::default());
        let expired = now() - TIMESTAMP_TOLERANCE_SECONDS - 60;

        let (status, body) = send(
            router(Arc::clone(&registration)),
            signed(event("eu-central-1_pool"), KEY, expired),
        )
        .await;

        assert_eq!(StatusCode::UNAUTHORIZED, status);
        assert_eq!("POST_CONFIRMATION_UNAUTHORIZED", body["error"]);
        assert!(registration.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_timestamp_from_the_future() {
        let registration = Arc::new(FakeRegistration::default());
        let future = now() + TIMESTAMP_TOLERANCE_SECONDS + 60;

        let (status, _) = send(
            router(Arc::clone(&registration)),
            signed(event("eu-central-1_pool"), KEY, future),
        )
        .await;

        assert_eq!(StatusCode::UNAUTHORIZED, status);
        assert!(registration.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_bad_signature() {
        let registration = Arc::new(FakeRegistration::default());
        let mut tampered = signed(event("eu-central-1_pool"), KEY, now());
        *tampered.body_mut() = Body::from(event("eu-central-1_other"));
        let unsigned = Request::post(PATH)
            .header("content-type", "application/json")
            .body(Body::from(event("eu-central-1_pool")))
            .unwrap();

        for request in [
            signed(
                event("eu-central-1_pool"),
                "a-different-signing-key-0123456789abc",
                now(),
            ),
            tampered,
            unsigned,
        ] {
            let (status, body) = send(router(Arc::clone(&registration)), request).await;
            assert_eq!(StatusCode::UNAUTHORIZED, status);
            assert_eq!("POST_CONFIRMATION_UNAUTHORIZED", body["error"]);
        }
        assert!(registration.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_signed_event_of_another_user_pool() {
        let registration = Arc::new(FakeRegistration::default());

        let (status, body) = send(
            router(Arc::clone(&registration)),
            signed(event("eu-central-1_other"), KEY, now()),
        )
        .await;

        assert_eq!(StatusCode::BAD_REQUEST, status);
        assert_eq!("POST_CONFIRMATION_INVALID_EVENT", body["error"]);
        assert!(registration.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_fail_the_confirmation_when_registration_fails() {
        for (failure, expected) in [
            (
                (|| RegisterCognitoUserError::TemporarilyUnavailable {
                    source: application::error::static_error("database down"),
                }) as Failure,
                StatusCode::SERVICE_UNAVAILABLE,
            ),
            (
                || RegisterCognitoUserError::IdentityConflict,
                StatusCode::CONFLICT,
            ),
            (
                || RegisterCognitoUserError::Internal {
                    source: application::error::static_error("internal"),
                },
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        ] {
            let registration = Arc::new(FakeRegistration {
                failure: Some(failure),
                ..FakeRegistration::default()
            });

            let (status, _) = send(
                router(registration),
                signed(event("eu-central-1_pool"), KEY, now()),
            )
            .await;

            assert_eq!(expected, status);
        }
    }
}
