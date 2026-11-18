//! Resolves the existing account of a federated sign-up for Cognito's pre-sign-up trigger, which
//! runs outside the host and cannot read the database. The trigger keeps the provider policy and
//! performs any Cognito link itself; this endpoint answers only the decision. The answer tells
//! whether an email has an account, so it requires a request signed for this purpose, and neither
//! the email nor the decision is logged. The trigger fails the sign-up on any non-2xx answer.
use crate::error::{
    ApiError, PRE_SIGN_UP_INTERNAL_ERROR, PRE_SIGN_UP_INVALID_REQUEST,
    PRE_SIGN_UP_PAYLOAD_TOO_LARGE, PRE_SIGN_UP_TEMPORARILY_UNAVAILABLE, PRE_SIGN_UP_UNAUTHORIZED,
};
use crate::internal::single_header;
use crate::state::CognitoPreSignUpState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cognito_post_confirmation::signature::{
    SIGNATURE_HEADER, SignatureError, SignaturePurpose, TIMESTAMP_HEADER,
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::time::Duration;
use tracing::Instrument;
use user_service::ports::CognitoIssuer;
use user_service::use_cases::{
    ExistingEmailAction, FederatedAccountDecision, ResolveFederatedAccountCommand,
    ResolveFederatedAccountError,
};

/// The request holds an email, an issuer and two flags.
pub(crate) const MAX_BODY_BYTES: usize = 4 * 1024;
/// The trigger gives up after 2.5 seconds, leaving time for its Cognito link call within
/// Cognito's five-second limit; answer before it does.
const PROCESSING_BUDGET: Duration = Duration::from_millis(2_000);

/// The federated sign-up the trigger resolves. The trigger has already matched its provider
/// against the provider policy and derived the issuer from the event's user pool. Unknown fields
/// are ignored, so a trigger may deploy a new field before the host reads it. The wire contract is
/// pinned by `docs/fixtures/cognito-pre-sign-up`, which the trigger's tests share.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreSignUpRequest {
    email: String,
    email_verified: bool,
    issuer: String,
    existing_email_action: ExistingEmailActionRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ExistingEmailActionRequest {
    LinkVerified,
    Reject,
}

impl From<ExistingEmailActionRequest> for ExistingEmailAction {
    fn from(action: ExistingEmailActionRequest) -> Self {
        match action {
            ExistingEmailActionRequest::LinkVerified => Self::LinkVerified,
            ExistingEmailActionRequest::Reject => Self::Reject,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision")]
enum PreSignUpDecisionResponse {
    #[serde(rename = "ALLOW_NEW_ACCOUNT")]
    Allow,
    #[serde(rename = "REJECT_EXISTING_ACCOUNT")]
    Reject,
    #[serde(rename = "LINK_EXISTING_ACCOUNT", rename_all = "camelCase")]
    Link {
        identity: CognitoIdentityResponse,
        canonical_email: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CognitoIdentityResponse {
    issuer: String,
    subject: String,
}

impl From<FederatedAccountDecision> for PreSignUpDecisionResponse {
    fn from(decision: FederatedAccountDecision) -> Self {
        match decision {
            FederatedAccountDecision::AllowNewAccount => Self::Allow,
            FederatedAccountDecision::RejectExistingAccount => Self::Reject,
            FederatedAccountDecision::LinkExistingAccount {
                identity,
                canonical_email,
            } => Self::Link {
                identity: CognitoIdentityResponse {
                    issuer: identity.issuer.as_str().to_owned(),
                    subject: identity.subject.as_str().to_owned(),
                },
                canonical_email: String::from(canonical_email),
            },
        }
    }
}

/// Why a signed request names no sign-up of the configured user pool. Messages never carry a
/// value.
#[derive(Debug, thiserror::Error)]
enum InvalidPreSignUpRequest {
    #[error("the request body is not a pre-sign-up request")]
    Malformed,
    #[error("invalid sign-up email")]
    InvalidEmail,
    #[error("invalid Cognito issuer")]
    InvalidIssuer,
    #[error("the request names a user pool other than the configured one")]
    ForeignUserPool,
}

pub async fn post_cognito_pre_sign_up(
    State(state): State<CognitoPreSignUpState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.len() > MAX_BODY_BYTES {
        return ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Payload Too Large",
            PRE_SIGN_UP_PAYLOAD_TOO_LARGE,
        )
        .into_response();
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if let Err(error) = state.verifier.verify(
        SignaturePurpose::PreSignUp,
        single_header(&headers, TIMESTAMP_HEADER),
        single_header(&headers, SIGNATURE_HEADER),
        &body,
        now,
    ) {
        return verification_error(error).into_response();
    }
    let command = match parse_command(&body, &state.expected_issuer) {
        Ok(command) => command,
        Err(error) => {
            tracing::warn!(
                event = "cognito_pre_sign_up.rejected",
                reason = %error,
                "Cognito pre-sign-up request rejected"
            );
            return ApiError::bad_request(PRE_SIGN_UP_INVALID_REQUEST)
                .with_detail("The pre-sign-up request is invalid.")
                .into_response();
        }
    };
    // The lookup runs in its own task, like post-confirmation's registration, so that neither the
    // budget nor a dropped connection cancels it while it holds a pooled connection.
    let resolve = state.resolve;
    let lookup = tokio::spawn(
        async move { resolve.execute(command).await }.instrument(tracing::Span::current()),
    );
    match tokio::time::timeout(PROCESSING_BUDGET, lookup).await {
        Ok(Ok(Ok(decision))) => Json(PreSignUpDecisionResponse::from(decision)).into_response(),
        Ok(Ok(Err(error))) => {
            let response = lookup_error(&error);
            tracing::warn!(
                event = "cognito_pre_sign_up.failed",
                status = response.status().as_u16(),
                error = %error,
                "Cognito pre-sign-up account lookup failed"
            );
            response
        }
        Ok(Err(_)) => {
            tracing::error!(
                event = "cognito_pre_sign_up.failed",
                status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                "Cognito pre-sign-up account lookup task panicked"
            );
            ApiError::internal_server_error(PRE_SIGN_UP_INTERNAL_ERROR).into_response()
        }
        Err(_) => {
            tracing::warn!(
                event = "cognito_pre_sign_up.failed",
                status = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                "Cognito pre-sign-up account lookup exceeded its processing budget"
            );
            temporarily_unavailable().into_response()
        }
    }
}

fn parse_command(
    body: &[u8],
    expected_issuer: &str,
) -> Result<ResolveFederatedAccountCommand, InvalidPreSignUpRequest> {
    let request: PreSignUpRequest =
        serde_json::from_slice(body).map_err(|_| InvalidPreSignUpRequest::Malformed)?;
    let current_issuer = CognitoIssuer::try_from(request.issuer)
        .map_err(|_| InvalidPreSignUpRequest::InvalidIssuer)?;
    if current_issuer.as_str() != expected_issuer {
        return Err(InvalidPreSignUpRequest::ForeignUserPool);
    }
    let email = Email::try_from(request.email.as_str())
        .map_err(|_| InvalidPreSignUpRequest::InvalidEmail)?;
    Ok(ResolveFederatedAccountCommand {
        email,
        email_verified: request.email_verified,
        current_issuer,
        existing_email_action: request.existing_email_action.into(),
    })
}

fn verification_error(error: SignatureError) -> ApiError {
    tracing::warn!(
        event = "cognito_pre_sign_up.unauthorized",
        reason = %error,
        "Cognito pre-sign-up request failed authentication"
    );
    match error {
        SignatureError::CryptographicFailure => {
            ApiError::internal_server_error(PRE_SIGN_UP_INTERNAL_ERROR)
        }
        SignatureError::MalformedTimestamp
        | SignatureError::StaleTimestamp
        | SignatureError::MalformedSignature
        | SignatureError::InvalidSignature => ApiError::unauthorized(PRE_SIGN_UP_UNAUTHORIZED)
            .with_detail("Request authentication failed."),
    }
}

fn lookup_error(error: &ResolveFederatedAccountError) -> Response {
    match error {
        ResolveFederatedAccountError::TemporarilyUnavailable => {
            temporarily_unavailable().into_response()
        }
        ResolveFederatedAccountError::InvalidPersistedState => {
            ApiError::internal_server_error(PRE_SIGN_UP_INTERNAL_ERROR).into_response()
        }
    }
}

fn temporarily_unavailable() -> ApiError {
    ApiError::service_unavailable(PRE_SIGN_UP_TEMPORARILY_UNAVAILABLE)
        .with_detail("The account lookup must be retried.")
}

#[cfg(test)]
mod tests {
    use crate::state::{AppState, CognitoPreSignUpState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use cognito_post_confirmation::signature::{
        SIGNATURE_HEADER, SignaturePurpose, SignatureVerifier, SigningKey, TIMESTAMP_HEADER,
        TIMESTAMP_TOLERANCE_SECONDS, sign,
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    use user_service::ports::{CognitoIdentity, CognitoIssuer, CognitoSubject};
    use user_service::use_cases::{
        ExistingEmailAction, FederatedAccountDecision, ResolveFederatedAccountCommand,
        ResolveFederatedAccountError, ResolveFederatedAccountUseCase,
    };

    const PATH: &str = "/internal/cognito/pre-sign-up";
    const KEY: &str = "pre-sign-up-test-signing-key-0123456789";
    const ISSUER: &str = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_pool";
    /// The trigger's tests send exactly this request.
    const REQUEST_FIXTURE: &str =
        include_str!("../../../../docs/fixtures/cognito-pre-sign-up/request.json");

    fn fixture(json: &str) -> Value {
        serde_json::from_str(json).unwrap()
    }

    struct FakeResolver {
        outcome: Result<FederatedAccountDecision, ResolveFederatedAccountError>,
        delay: Option<std::time::Duration>,
        commands: Mutex<Vec<ResolveFederatedAccountCommand>>,
    }

    impl FakeResolver {
        fn answering(
            outcome: Result<FederatedAccountDecision, ResolveFederatedAccountError>,
        ) -> Arc<Self> {
            Arc::new(Self {
                outcome,
                delay: None,
                commands: Mutex::new(Vec::new()),
            })
        }

        fn commands(&self) -> Vec<ResolveFederatedAccountCommand> {
            self.commands.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ResolveFederatedAccountUseCase for FakeResolver {
        async fn execute(
            &self,
            command: ResolveFederatedAccountCommand,
        ) -> Result<FederatedAccountDecision, ResolveFederatedAccountError> {
            self.commands.lock().unwrap().push(command);
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.outcome.clone()
        }
    }

    fn router(resolver: Arc<FakeResolver>) -> axum::Router {
        crate::app(
            AppState::new().with_cognito_pre_sign_up(CognitoPreSignUpState::new(
                SignatureVerifier::new(SigningKey::new(KEY).unwrap(), None),
                ISSUER,
                resolver,
            )),
        )
    }

    fn request_body(issuer: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "email": "ada@example.test",
            "emailVerified": true,
            "issuer": issuer,
            "existingEmailAction": "LINK_VERIFIED"
        }))
        .unwrap()
    }

    fn now() -> i64 {
        time::OffsetDateTime::now_utc().unix_timestamp()
    }

    fn signed_for(
        purpose: SignaturePurpose,
        body: Vec<u8>,
        key: &str,
        timestamp: i64,
    ) -> Request<Body> {
        let signature = sign(&SigningKey::new(key).unwrap(), purpose, timestamp, &body).unwrap();
        Request::post(PATH)
            .header("content-type", "application/json")
            .header(TIMESTAMP_HEADER, timestamp.to_string())
            .header(SIGNATURE_HEADER, signature)
            .body(Body::from(body))
            .unwrap()
    }

    fn signed(body: Vec<u8>, timestamp: i64) -> Request<Body> {
        signed_for(SignaturePurpose::PreSignUp, body, KEY, timestamp)
    }

    async fn send(router: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn identity() -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from(ISSUER).unwrap(),
            subject: CognitoSubject::try_from("canonical-subject").unwrap(),
        }
    }

    #[tokio::test]
    async fn should_answer_each_decision_of_a_validly_signed_request() {
        for (decision, expected) in [
            (
                FederatedAccountDecision::AllowNewAccount,
                fixture(include_str!(
                    "../../../../docs/fixtures/cognito-pre-sign-up/decision-allow.json"
                )),
            ),
            (
                FederatedAccountDecision::RejectExistingAccount,
                fixture(include_str!(
                    "../../../../docs/fixtures/cognito-pre-sign-up/decision-reject.json"
                )),
            ),
            (
                FederatedAccountDecision::LinkExistingAccount {
                    identity: identity(),
                    canonical_email: "ada@example.test".try_into().unwrap(),
                },
                fixture(include_str!(
                    "../../../../docs/fixtures/cognito-pre-sign-up/decision-link.json"
                )),
            ),
        ] {
            let resolver = FakeResolver::answering(Ok(decision));

            let (status, body) = send(
                router(Arc::clone(&resolver)),
                signed(REQUEST_FIXTURE.as_bytes().to_vec(), now()),
            )
            .await;

            assert_eq!(StatusCode::OK, status);
            assert_eq!(expected, body);
            let commands = resolver.commands();
            assert_eq!(1, commands.len());
            assert_eq!("ada@example.test", AsRef::<str>::as_ref(&commands[0].email));
            assert!(commands[0].email_verified);
            assert_eq!(ISSUER, commands[0].current_issuer.as_str());
            assert_eq!(
                ExistingEmailAction::LinkVerified,
                commands[0].existing_email_action
            );
        }
    }

    #[tokio::test]
    async fn should_refuse_a_replayed_or_future_timestamp() {
        let resolver = FakeResolver::answering(Ok(FederatedAccountDecision::AllowNewAccount));

        for timestamp in [
            now() - TIMESTAMP_TOLERANCE_SECONDS - 60,
            now() + TIMESTAMP_TOLERANCE_SECONDS + 60,
        ] {
            let (status, body) = send(
                router(Arc::clone(&resolver)),
                signed(request_body(ISSUER), timestamp),
            )
            .await;

            assert_eq!(StatusCode::UNAUTHORIZED, status);
            assert_eq!("PRE_SIGN_UP_UNAUTHORIZED", body["error"]);
        }
        assert!(resolver.commands().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_bad_signature_or_one_made_for_another_purpose() {
        let resolver = FakeResolver::answering(Ok(FederatedAccountDecision::AllowNewAccount));
        let mut tampered = signed(request_body(ISSUER), now());
        *tampered.body_mut() = Body::from(
            serde_json::to_vec(&json!({
                "email": "eve@example.test",
                "emailVerified": true,
                "issuer": ISSUER,
                "existingEmailAction": "LINK_VERIFIED"
            }))
            .unwrap(),
        );
        let unsigned = Request::post(PATH)
            .header("content-type", "application/json")
            .body(Body::from(request_body(ISSUER)))
            .unwrap();

        for request in [
            signed_for(
                SignaturePurpose::PreSignUp,
                request_body(ISSUER),
                "a-different-signing-key-0123456789abc",
                now(),
            ),
            signed_for(
                SignaturePurpose::PostConfirmation,
                request_body(ISSUER),
                KEY,
                now(),
            ),
            tampered,
            unsigned,
        ] {
            let (status, body) = send(router(Arc::clone(&resolver)), request).await;
            assert_eq!(StatusCode::UNAUTHORIZED, status);
            assert_eq!("PRE_SIGN_UP_UNAUTHORIZED", body["error"]);
        }
        assert!(resolver.commands().is_empty());
    }

    #[tokio::test]
    async fn should_refuse_a_signed_request_of_another_user_pool_or_a_malformed_one() {
        let resolver = FakeResolver::answering(Ok(FederatedAccountDecision::AllowNewAccount));
        let invalid_email = serde_json::to_vec(&json!({
            "email": "not-an-email",
            "emailVerified": false,
            "issuer": ISSUER,
            "existingEmailAction": "REJECT"
        }))
        .unwrap();
        let unknown_action = serde_json::to_vec(&json!({
            "email": "ada@example.test",
            "emailVerified": true,
            "issuer": ISSUER,
            "existingEmailAction": "LINK_ANY"
        }))
        .unwrap();

        for body in [
            request_body("https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_other"),
            invalid_email,
            unknown_action,
            b"{}".to_vec(),
        ] {
            let (status, body) = send(router(Arc::clone(&resolver)), signed(body, now())).await;

            assert_eq!(StatusCode::BAD_REQUEST, status);
            assert_eq!("PRE_SIGN_UP_INVALID_REQUEST", body["error"]);
        }
        assert!(resolver.commands().is_empty());
    }

    #[tokio::test]
    async fn should_fail_the_sign_up_when_the_lookup_fails() {
        for (failure, expected_status, expected_code) in [
            (
                ResolveFederatedAccountError::TemporarilyUnavailable,
                StatusCode::SERVICE_UNAVAILABLE,
                "PRE_SIGN_UP_TEMPORARILY_UNAVAILABLE",
            ),
            (
                ResolveFederatedAccountError::InvalidPersistedState,
                StatusCode::INTERNAL_SERVER_ERROR,
                "PRE_SIGN_UP_INTERNAL_ERROR",
            ),
        ] {
            let resolver = FakeResolver::answering(Err(failure));

            let (status, body) = send(router(resolver), signed(request_body(ISSUER), now())).await;

            assert_eq!(expected_status, status);
            assert_eq!(expected_code, body["error"]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_answer_unavailable_when_the_lookup_exceeds_its_budget() {
        let resolver = Arc::new(FakeResolver {
            outcome: Ok(FederatedAccountDecision::AllowNewAccount),
            delay: Some(super::PROCESSING_BUDGET + std::time::Duration::from_secs(1)),
            commands: Mutex::new(Vec::new()),
        });

        let (status, body) = send(router(resolver), signed(request_body(ISSUER), now())).await;

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, status);
        assert_eq!("PRE_SIGN_UP_TEMPORARILY_UNAVAILABLE", body["error"]);
    }
}
