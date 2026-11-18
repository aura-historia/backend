//! The federated-account lookup after the move to the host: the trigger runs outside any VPC and
//! cannot read the database, so it asks the host's internal endpoint for the decision, signed for
//! this purpose and through the public API hostname. The request carries only what the decision
//! needs; neither it nor the answer is logged, because together they tell whether an email has an
//! account. Any failure fails the sign-up, as a database error did.
use cognito_trigger_signing::{
    signature::{SignaturePurpose, SigningKey},
    signed_request::{SignedHostClient, SignedHostClientConfigError, SignedRequestError},
};
use serde::{Deserialize, Serialize};
use serde_email::Email;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use user_service::{
    ports::{CognitoIdentity, CognitoIssuer, CognitoSubject},
    use_cases::{
        ExistingEmailAction, FederatedAccountDecision, ResolveFederatedAccountCommand,
        ResolveFederatedAccountError, ResolveFederatedAccountUseCase,
    },
};

pub const ENDPOINT_URL_ENV: &str = "PRE_SIGN_UP_ENDPOINT_URL";
pub const SIGNING_KEY_ENV: &str = "PRE_SIGN_UP_SIGNING_KEY";
/// Leaves the Cognito link call room within the invocation deadline and Cognito's five seconds.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_millis(2_500);
/// A decision is a few hundred bytes; a larger answer is not one.
const MAX_RESPONSE_BYTES: usize = 4 * 1024;

/// Safe to display: no configuration value is retained.
#[derive(Debug, thiserror::Error)]
pub enum HostLookupConfigError {
    #[error("missing required configuration {name}")]
    Missing { name: &'static str },
    #[error("{ENDPOINT_URL_ENV} must be an HTTPS URL, or HTTP on a loopback host")]
    InvalidEndpoint,
    #[error("{SIGNING_KEY_ENV} is too short")]
    WeakSigningKey,
    #[error("failed to build the HTTP client")]
    Client(#[source] reqwest::Error),
}

impl From<SignedHostClientConfigError> for HostLookupConfigError {
    fn from(error: SignedHostClientConfigError) -> Self {
        match error {
            SignedHostClientConfigError::InvalidEndpoint => Self::InvalidEndpoint,
            SignedHostClientConfigError::Client(source) => Self::Client(source),
        }
    }
}

/// The host's pre-sign-up endpoint, shared by every invocation of an execution environment.
pub struct HostAccountLookup {
    client: SignedHostClient,
}

impl HostAccountLookup {
    pub fn new(endpoint: &str, key: SigningKey) -> Result<Self, HostLookupConfigError> {
        Ok(Self {
            client: SignedHostClient::new(
                endpoint,
                key,
                SignaturePurpose::PreSignUp,
                LOOKUP_TIMEOUT,
            )?,
        })
    }

    pub fn from_env() -> Result<Self, HostLookupConfigError> {
        let setting = |name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or(HostLookupConfigError::Missing { name })
        };
        let key = SigningKey::new(setting(SIGNING_KEY_ENV)?)
            .map_err(|_| HostLookupConfigError::WeakSigningKey)?;
        Self::new(&setting(ENDPOINT_URL_ENV)?, key)
    }

    /// The resolver of one invocation; `request_id` ties the host's logs to it.
    pub fn for_invocation<'a>(&'a self, request_id: &'a str) -> HostFederatedAccountResolver<'a> {
        HostFederatedAccountResolver {
            lookup: self,
            request_id,
        }
    }
}

pub struct HostFederatedAccountResolver<'a> {
    lookup: &'a HostAccountLookup,
    request_id: &'a str,
}

#[async_trait::async_trait]
impl ResolveFederatedAccountUseCase for HostFederatedAccountResolver<'_> {
    async fn execute(
        &self,
        command: ResolveFederatedAccountCommand,
    ) -> Result<FederatedAccountDecision, ResolveFederatedAccountError> {
        self.lookup(&command).await.map_err(|failure| {
            tracing::warn!(
                request_id = self.request_id,
                event = "cognito_pre_sign_up.host_lookup_failed",
                result_category = failure.category(),
                status = failure.status(),
            );
            failure.into()
        })
    }
}

impl HostFederatedAccountResolver<'_> {
    async fn lookup(
        &self,
        command: &ResolveFederatedAccountCommand,
    ) -> Result<FederatedAccountDecision, LookupFailure> {
        let body = serde_json::to_vec(&LookupRequest::from(command))
            .map_err(|_| LookupFailure::Request)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| LookupFailure::Request)?
            .as_secs()
            .cast_signed();
        let response = self
            .lookup
            .client
            .post_json(body, now, self.request_id)
            .await
            .map_err(LookupFailure::from)?;
        let body = read_bounded(response).await?;
        serde_json::from_slice::<DecisionResponse>(&body)
            .map_err(|_| LookupFailure::MalformedDecision)?
            .try_into()
    }
}

/// Why the host gave no decision. Carries no request or answer content.
#[derive(Debug)]
enum LookupFailure {
    Request,
    Transport,
    Refused { status: u16 },
    MalformedDecision,
}

impl LookupFailure {
    const fn category(&self) -> &'static str {
        match self {
            Self::Request => "request_failed",
            Self::Transport => "transport_failed",
            Self::Refused { .. } => "refused",
            Self::MalformedDecision => "malformed_decision",
        }
    }

    const fn status(&self) -> Option<u16> {
        match self {
            Self::Refused { status } => Some(*status),
            Self::Request | Self::Transport | Self::MalformedDecision => None,
        }
    }
}

impl From<SignedRequestError> for LookupFailure {
    fn from(error: SignedRequestError) -> Self {
        match error {
            SignedRequestError::Sign(_) => Self::Request,
            SignedRequestError::Transport(_) => Self::Transport,
            SignedRequestError::Refused { status } => Self::Refused { status },
        }
    }
}

/// A host that answers, but not with a decision, is in a state the trigger must not act on.
impl From<LookupFailure> for ResolveFederatedAccountError {
    fn from(failure: LookupFailure) -> Self {
        match failure {
            LookupFailure::Request | LookupFailure::Transport | LookupFailure::Refused { .. } => {
                Self::TemporarilyUnavailable
            }
            LookupFailure::MalformedDecision => Self::InvalidPersistedState,
        }
    }
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, LookupFailure> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LookupFailure::Transport)?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(LookupFailure::MalformedDecision);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LookupRequest<'a> {
    email: &'a str,
    email_verified: bool,
    issuer: &'a str,
    existing_email_action: &'static str,
}

impl<'a> From<&'a ResolveFederatedAccountCommand> for LookupRequest<'a> {
    fn from(command: &'a ResolveFederatedAccountCommand) -> Self {
        Self {
            email: command.email.as_ref(),
            email_verified: command.email_verified,
            issuer: command.current_issuer.as_str(),
            existing_email_action: match command.existing_email_action {
                ExistingEmailAction::LinkVerified => "LINK_VERIFIED",
                ExistingEmailAction::Reject => "REJECT",
            },
        }
    }
}

/// Unknown fields are ignored so the host can extend the answer; unknown decisions are refused.
#[derive(Deserialize)]
#[serde(tag = "decision")]
enum DecisionResponse {
    #[serde(rename = "ALLOW_NEW_ACCOUNT")]
    Allow,
    #[serde(rename = "REJECT_EXISTING_ACCOUNT")]
    Reject,
    #[serde(rename = "LINK_EXISTING_ACCOUNT", rename_all = "camelCase")]
    Link {
        identity: IdentityResponse,
        canonical_email: String,
    },
}

#[derive(Deserialize)]
struct IdentityResponse {
    issuer: String,
    subject: String,
}

impl TryFrom<DecisionResponse> for FederatedAccountDecision {
    type Error = LookupFailure;

    fn try_from(response: DecisionResponse) -> Result<Self, Self::Error> {
        Ok(match response {
            DecisionResponse::Allow => Self::AllowNewAccount,
            DecisionResponse::Reject => Self::RejectExistingAccount,
            DecisionResponse::Link {
                identity,
                canonical_email,
            } => Self::LinkExistingAccount {
                identity: CognitoIdentity {
                    issuer: CognitoIssuer::try_from(identity.issuer)
                        .map_err(|_| LookupFailure::MalformedDecision)?,
                    subject: CognitoSubject::try_from(identity.subject)
                        .map_err(|_| LookupFailure::MalformedDecision)?,
                },
                canonical_email: Email::try_from(canonical_email)
                    .map_err(|_| LookupFailure::MalformedDecision)?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::post,
    };
    use cognito_trigger_signing::{
        signature::{SIGNATURE_HEADER, SignatureVerifier, TIMESTAMP_HEADER},
        signed_request::CORRELATION_ID_HEADER,
    };
    use std::sync::{Arc, Mutex};

    const KEY: &str = "pre-sign-up-lookup-test-signing-key-0123";
    const ISSUER: &str = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_pool";
    // The host's endpoint tests answer and accept exactly these documents.
    const REQUEST_FIXTURE: &str =
        include_str!("../../../docs/fixtures/cognito-pre-sign-up/request.json");
    const ALLOW_FIXTURE: &str =
        include_str!("../../../docs/fixtures/cognito-pre-sign-up/decision-allow.json");
    const REJECT_FIXTURE: &str =
        include_str!("../../../docs/fixtures/cognito-pre-sign-up/decision-reject.json");
    const LINK_FIXTURE: &str =
        include_str!("../../../docs/fixtures/cognito-pre-sign-up/decision-link.json");

    type Received = Arc<Mutex<Vec<(HeaderMap, Bytes)>>>;

    /// A host endpoint that records each request and answers `status` with `body`, or never
    /// answers.
    async fn host(answer: Option<(StatusCode, String)>) -> (String, Received) {
        let received = Received::default();
        let app = Router::new()
            .route(
                "/internal/cognito/pre-sign-up",
                post(
                    move |State(received): State<Received>, headers: HeaderMap, body: Bytes| async move {
                        received.lock().unwrap().push((headers, body));
                        match answer {
                            Some((status, body)) => {
                                (status, [(header::CONTENT_TYPE, "application/json")], body)
                                    .into_response()
                            }
                            None => std::future::pending().await,
                        }
                    },
                ),
            )
            .with_state(Arc::clone(&received));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await });
        (
            format!("http://{address}/internal/cognito/pre-sign-up"),
            received,
        )
    }

    async fn resolve(
        answer: Option<(StatusCode, String)>,
    ) -> (
        Result<FederatedAccountDecision, ResolveFederatedAccountError>,
        Received,
    ) {
        let (endpoint, received) = host(answer).await;
        let lookup = HostAccountLookup::new(&endpoint, SigningKey::new(KEY).unwrap()).unwrap();
        let outcome = lookup
            .for_invocation("lambda-request-id")
            .execute(command())
            .await;
        (outcome, received)
    }

    fn fixture(body: &str) -> Option<(StatusCode, String)> {
        Some((StatusCode::OK, body.to_owned()))
    }

    fn command() -> ResolveFederatedAccountCommand {
        ResolveFederatedAccountCommand {
            email: "ada@example.test".try_into().unwrap(),
            email_verified: true,
            current_issuer: CognitoIssuer::try_from(ISSUER).unwrap(),
            existing_email_action: ExistingEmailAction::LinkVerified,
        }
    }

    #[tokio::test]
    async fn should_sign_the_lookup_for_its_purpose_and_send_only_the_decision_inputs() {
        let (outcome, received) = resolve(fixture(ALLOW_FIXTURE)).await;

        assert_eq!(Ok(FederatedAccountDecision::AllowNewAccount), outcome);
        let received = received.lock().unwrap();
        let (headers, body) = &received[0];
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(REQUEST_FIXTURE).unwrap(),
            serde_json::from_slice::<serde_json::Value>(body).unwrap()
        );
        assert_eq!("lambda-request-id", headers[CORRELATION_ID_HEADER]);
        let timestamp = headers[TIMESTAMP_HEADER].to_str().unwrap().parse().unwrap();
        let verifier = SignatureVerifier::new(SigningKey::new(KEY).unwrap(), None);
        let verify = |purpose| {
            verifier.verify(
                purpose,
                Some(headers[TIMESTAMP_HEADER].as_bytes()),
                Some(headers[SIGNATURE_HEADER].as_bytes()),
                body,
                timestamp,
            )
        };
        assert_eq!(Ok(()), verify(SignaturePurpose::PreSignUp));
        assert!(verify(SignaturePurpose::PostConfirmation).is_err());
    }

    #[tokio::test]
    async fn should_map_each_decision_of_the_host() {
        let (rejected, _) = resolve(fixture(REJECT_FIXTURE)).await;
        let (linked, _) = resolve(fixture(LINK_FIXTURE)).await;

        assert_eq!(
            Ok(FederatedAccountDecision::RejectExistingAccount),
            rejected
        );
        assert_eq!(
            Ok(FederatedAccountDecision::LinkExistingAccount {
                identity: CognitoIdentity {
                    issuer: CognitoIssuer::try_from(ISSUER).unwrap(),
                    subject: CognitoSubject::try_from("canonical-subject").unwrap(),
                },
                canonical_email: "ada@example.test".try_into().unwrap(),
            }),
            linked
        );
    }

    #[tokio::test]
    async fn should_fail_closed_when_the_host_refuses_the_lookup() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::BAD_REQUEST,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let (outcome, _) = resolve(Some((status, "{}".to_owned()))).await;

            assert_eq!(
                Err(ResolveFederatedAccountError::TemporarilyUnavailable),
                outcome
            );
        }
    }

    #[tokio::test]
    async fn should_fail_closed_when_the_host_does_not_answer_in_time() {
        let started = tokio::time::Instant::now();

        let (outcome, received) = resolve(None).await;

        assert_eq!(
            Err(ResolveFederatedAccountError::TemporarilyUnavailable),
            outcome
        );
        assert!(started.elapsed() < LOOKUP_TIMEOUT + Duration::from_secs(1));
        assert_eq!(1, received.lock().unwrap().len());
    }

    #[tokio::test]
    async fn should_fail_closed_on_an_answer_that_is_not_a_valid_decision() {
        for body in [
            "not json".to_owned(),
            serde_json::json!({}).to_string(),
            serde_json::json!({ "decision": "LINK_ANY_ACCOUNT" }).to_string(),
            serde_json::json!({ "decision": "LINK_EXISTING_ACCOUNT" }).to_string(),
            serde_json::json!({
                "decision": "LINK_EXISTING_ACCOUNT",
                "identity": { "issuer": ISSUER, "subject": "" },
                "canonicalEmail": "ada@example.test"
            })
            .to_string(),
            serde_json::json!({
                "decision": "LINK_EXISTING_ACCOUNT",
                "identity": { "issuer": ISSUER, "subject": "canonical-subject" },
                "canonicalEmail": "not-an-email"
            })
            .to_string(),
            format!(
                r#"{{"decision":"ALLOW_NEW_ACCOUNT","padding":"{}"}}"#,
                "x".repeat(MAX_RESPONSE_BYTES)
            ),
        ] {
            let (outcome, _) = resolve(Some((StatusCode::OK, body))).await;

            assert_eq!(
                Err(ResolveFederatedAccountError::InvalidPersistedState),
                outcome
            );
        }
    }
}
