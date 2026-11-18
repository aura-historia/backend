//! Cognito's post-confirmation trigger after the move to the host: it holds no business state and
//! forwards each confirmation event, signed and timestamped, to the host's internal endpoint
//! through the public API hostname. A failed or refused forward fails the confirmation, as a
//! failed registration did when the trigger wrote to the database itself.
use cognito_post_confirmation::signature::{
    SIGNATURE_HEADER, SignatureError, SigningKey, TIMESTAMP_HEADER, sign,
};
use std::time::Duration;
use url::{Host, Url};

pub const ENDPOINT_URL_ENV: &str = "POST_CONFIRMATION_ENDPOINT_URL";
pub const SIGNING_KEY_ENV: &str = "POST_CONFIRMATION_SIGNING_KEY";
/// Cognito waits at most five seconds for the trigger, which also has to start and answer.
pub const FORWARD_TIMEOUT: Duration = Duration::from_millis(4_200);
const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// Safe to display: no configuration value is retained.
#[derive(Debug, thiserror::Error)]
pub enum ForwarderConfigError {
    #[error("missing required configuration {name}")]
    Missing { name: &'static str },
    #[error("{ENDPOINT_URL_ENV} must be an HTTPS URL, or HTTP on a loopback host")]
    InvalidEndpoint,
    #[error("{SIGNING_KEY_ENV} is too short")]
    WeakSigningKey,
    #[error("failed to build the HTTP client")]
    Client(#[source] reqwest::Error),
}

/// Why a confirmation was not forwarded. Never carries the event, which holds the user's email.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("failed to serialize the confirmation event")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to sign the confirmation event")]
    Sign(#[source] SignatureError),
    #[error("the host endpoint could not be reached in time")]
    Transport(#[source] reqwest::Error),
    #[error("the host endpoint refused the confirmation with status {status}")]
    Refused { status: u16 },
}

impl ForwardError {
    /// A bounded label for the trigger's failure log.
    pub const fn category(&self) -> &'static str {
        match self {
            Self::Serialize(_) => "serialize_failed",
            Self::Sign(_) => "sign_failed",
            Self::Transport(_) => "transport_failed",
            Self::Refused { .. } => "refused",
        }
    }
}

pub struct Forwarder {
    client: reqwest::Client,
    endpoint: Url,
    key: SigningKey,
}

impl Forwarder {
    pub fn new(endpoint: &str, key: SigningKey) -> Result<Self, ForwarderConfigError> {
        let endpoint = trusted_endpoint(endpoint).ok_or(ForwarderConfigError::InvalidEndpoint)?;
        let client = reqwest::Client::builder()
            .timeout(FORWARD_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ForwarderConfigError::Client)?;
        Ok(Self {
            client,
            endpoint,
            key,
        })
    }

    pub fn from_env() -> Result<Self, ForwarderConfigError> {
        let setting = |name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or(ForwarderConfigError::Missing { name })
        };
        let key = SigningKey::new(setting(SIGNING_KEY_ENV)?)
            .map_err(|_| ForwarderConfigError::WeakSigningKey)?;
        Self::new(&setting(ENDPOINT_URL_ENV)?, key)
    }

    /// Posts `event` signed at `now` (Unix seconds) and succeeds only on a 2xx answer.
    /// `correlation_id` (the invocation's request ID) ties the host's logs to the invocation.
    pub async fn forward(
        &self,
        event: &serde_json::Value,
        now: i64,
        correlation_id: &str,
    ) -> Result<(), ForwardError> {
        let body = serde_json::to_vec(event).map_err(ForwardError::Serialize)?;
        let signature = sign(&self.key, now, &body).map_err(ForwardError::Sign)?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(TIMESTAMP_HEADER, now.to_string())
            .header(SIGNATURE_HEADER, signature)
            .header(CORRELATION_ID_HEADER, correlation_id)
            .body(body)
            .send()
            .await
            .map_err(|error| ForwardError::Transport(error.without_url()))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(ForwardError::Refused {
                status: status.as_u16(),
            })
        }
    }
}

fn trusted_endpoint(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let loopback = match url.host()? {
        Host::Domain(domain) => domain == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    };
    let trusted_scheme = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    (trusted_scheme
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
    };
    use cognito_post_confirmation::signature::SignatureVerifier;
    use std::sync::{Arc, Mutex};

    const KEY: &str = "forwarder-test-signing-key-0123456789";
    const NOW: i64 = 1_800_000_000;

    type Received = Arc<Mutex<Vec<(HeaderMap, Bytes)>>>;

    /// A host endpoint that records each request and answers `status`, or never answers.
    async fn host(status: Option<StatusCode>) -> (String, Received) {
        let received = Received::default();
        let app = Router::new()
            .route(
                "/internal/cognito/post-confirmation",
                post(
                    move |State(received): State<Received>, headers: HeaderMap, body: Bytes| async move {
                        received.lock().unwrap().push((headers, body));
                        match status {
                            Some(status) => status,
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
            format!("http://{address}/internal/cognito/post-confirmation"),
            received,
        )
    }

    fn forwarder(endpoint: &str) -> Forwarder {
        Forwarder::new(endpoint, SigningKey::new(KEY).unwrap()).unwrap()
    }

    fn event() -> serde_json::Value {
        serde_json::json!({
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "eu-central-1",
            "userPoolId": "eu-central-1_pool",
            "request": { "userAttributes": { "sub": "subject", "email": "ada@example.test" } },
            "response": {}
        })
    }

    #[tokio::test]
    async fn should_post_the_event_signed_so_the_host_verifier_accepts_it() {
        let (endpoint, received) = host(Some(StatusCode::NO_CONTENT)).await;

        forwarder(&endpoint)
            .forward(&event(), NOW, "lambda-request-id")
            .await
            .unwrap();

        let received = received.lock().unwrap();
        let (headers, body) = &received[0];
        assert_eq!(
            event(),
            serde_json::from_slice::<serde_json::Value>(body).unwrap()
        );
        assert_eq!("lambda-request-id", headers[CORRELATION_ID_HEADER]);
        assert_eq!(
            Ok(()),
            SignatureVerifier::new(SigningKey::new(KEY).unwrap(), None).verify(
                headers.get(TIMESTAMP_HEADER).map(|value| value.as_bytes()),
                headers.get(SIGNATURE_HEADER).map(|value| value.as_bytes()),
                body,
                NOW,
            )
        );
    }

    #[tokio::test]
    async fn should_fail_the_confirmation_when_the_host_refuses_it() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::SERVICE_UNAVAILABLE] {
            let (endpoint, _) = host(Some(status)).await;

            assert!(matches!(
                forwarder(&endpoint).forward(&event(), NOW, "id").await,
                Err(ForwardError::Refused { status: refused }) if refused == status.as_u16()
            ));
        }
    }

    #[tokio::test]
    async fn should_fail_the_confirmation_when_the_host_does_not_answer_in_time() {
        let (endpoint, received) = host(None).await;
        let started = tokio::time::Instant::now();

        let outcome = forwarder(&endpoint).forward(&event(), NOW, "id").await;

        assert!(matches!(outcome, Err(ForwardError::Transport(_))));
        assert!(started.elapsed() < FORWARD_TIMEOUT + Duration::from_secs(1));
        assert_eq!(1, received.lock().unwrap().len());
    }

    #[test]
    fn should_accept_only_https_or_loopback_http_endpoints() {
        for endpoint in [
            "https://api.aura-historia.com/internal/cognito/post-confirmation",
            "http://127.0.0.1:8080/internal/cognito/post-confirmation",
            "http://localhost:8080/internal/cognito/post-confirmation",
        ] {
            assert!(trusted_endpoint(endpoint).is_some(), "{endpoint}");
        }
        for endpoint in [
            "",
            "api.aura-historia.com/internal",
            "http://api.aura-historia.com/internal/cognito/post-confirmation",
            "https://user:secret@api.aura-historia.com/internal",
            "https://api.aura-historia.com/internal?next=x",
            "https://api.aura-historia.com/internal#x",
        ] {
            assert!(trusted_endpoint(endpoint).is_none(), "{endpoint}");
        }
    }
}
