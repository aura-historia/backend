use crate::IntegrationTestService;
use crate::localstack::{get_aws_config, get_endpoint_url};
use async_trait::async_trait;
use aws_sdk_sesv2::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

/// The sender email address used by all notification Lambdas in the
/// ephemeral LocalStack stack.
///
/// Must match the `SENDER_MAIL` environment variable set in
/// `cfn/ephemeral.yaml`.
const SENDER_EMAIL: &str = "no-reply@notify.aura-historia.com";

/// A lazily-initialized, globally shared SESv2 client for integration testing.
static SES_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `aws_sdk_sesv2::Client` for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
pub async fn get_ses_client() -> &'static Client {
    let client = SES_CLIENT
        .get_or_init(|| async { Client::new(get_aws_config().await) })
        .await;
    debug!("Successfully initialized SESv2-Client.");
    client
}

/// Marker type representing the SES service in LocalStack-based tests.
///
/// Implements [`IntegrationTestService`] for use with the `#[localstack_test]` macro.
///
/// On [`set_up`](IntegrationTestService::set_up), verifies the sender email identity
/// (`no-reply@notify.aura-historia.com`) so that Lambdas running inside LocalStack can
/// send emails via SES without identity-not-verified errors.
pub struct Ses();

#[async_trait]
impl IntegrationTestService for Ses {
    fn service_names(&self) -> &'static [&'static str] {
        &["sesv2"]
    }

    async fn set_up(&self) {
        verify_sender_identity(SENDER_EMAIL).await;
    }

    async fn tear_down(&self) {
        clear_sent_emails().await;
    }
}

// ---------------------------------------------------------------------------
// SES identity verification
// ---------------------------------------------------------------------------

/// Verifies an email identity in LocalStack SES so that it can be used as a
/// sender address.
///
/// On real AWS this would trigger a verification email; LocalStack marks the
/// identity as verified immediately.
pub async fn verify_sender_identity(email: &str) {
    let client = get_ses_client().await;
    client
        .create_email_identity()
        .email_identity(email)
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail verifying SES email identity '{email}': {e}"));
    info!("Verified SES sender identity '{email}'.");
}

// ---------------------------------------------------------------------------
// LocalStack /_aws/ses introspection types
// ---------------------------------------------------------------------------

/// Response shape returned by LocalStack's `GET /_aws/ses` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SentEmailsResponse {
    pub messages: Vec<SentEmail>,
}

/// A single email captured by LocalStack's in-memory SES store.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SentEmail {
    pub id: String,
    pub region: String,
    pub destination: SentEmailDestination,
    pub source: String,
    pub subject: String,
    pub body: SentEmailBody,
    pub timestamp: String,
}

/// The destination addresses of a sent email.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SentEmailDestination {
    pub to_addresses: Vec<String>,
}

/// The body parts of a sent email.
#[derive(Debug, Clone, Deserialize)]
pub struct SentEmailBody {
    pub text_part: Option<String>,
    pub html_part: Option<String>,
}

// ---------------------------------------------------------------------------
// Helper functions for querying sent emails
// ---------------------------------------------------------------------------

/// Retrieves all emails sent through LocalStack SES.
///
/// Queries the `/_aws/ses` internal endpoint which LocalStack exposes for
/// retrospecting sent messages.
pub async fn get_sent_emails() -> Vec<SentEmail> {
    let url = format!("{}/_aws/ses", get_endpoint_url());
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail querying LocalStack SES endpoint: {e}"));

    let body = response
        .json::<SentEmailsResponse>()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail deserializing SES messages response: {e}"));

    body.messages
}

/// Retrieves emails sent through LocalStack SES filtered by source email.
///
/// Uses the `email` query parameter on the `/_aws/ses` endpoint.
pub async fn get_sent_emails_from(source_email: &str) -> Vec<SentEmail> {
    let url = format!("{}/_aws/ses", get_endpoint_url());
    let response = reqwest::Client::new()
        .get(&url)
        .query(&[("email", source_email)])
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail querying LocalStack SES endpoint: {e}"));

    let body = response
        .json::<SentEmailsResponse>()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail deserializing SES messages response: {e}"));

    body.messages
}

/// Deletes all sent emails from LocalStack's in-memory SES store.
///
/// Intended for test isolation — call this in `tear_down` so that each test
/// starts with a clean slate.
pub async fn clear_sent_emails() {
    let url = format!("{}/_aws/ses", get_endpoint_url());
    reqwest::Client::new()
        .delete(&url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail clearing LocalStack SES messages: {e}"));
    debug!("Cleared all sent emails from LocalStack SES.");
}

/// Polls LocalStack SES for an email whose subject contains the given string.
///
/// Retries every 2 seconds for up to `timeout` duration.
///
/// # Returns
///
/// `true` if a matching email was found within the timeout, `false` otherwise.
pub async fn wait_for_ses_email(subject_contains: &str, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    let poll_interval = Duration::from_secs(2);

    loop {
        let emails = get_sent_emails().await;
        if let Some(matched) = emails.iter().find(|e| e.subject.contains(subject_contains)) {
            info!(
                subject = %matched.subject,
                to = ?matched.destination.to_addresses,
                "Found matching email in LocalStack SES."
            );
            return true;
        }

        if start.elapsed() >= timeout {
            warn!(
                subject_contains,
                elapsed_secs = start.elapsed().as_secs(),
                total_emails = emails.len(),
                "Timed out waiting for email."
            );
            return false;
        }

        debug!(
            subject_contains,
            total_emails = emails.len(),
            "No matching email yet, retrying..."
        );
        tokio::time::sleep(poll_interval).await;
    }
}

/// Polls LocalStack SES for an email whose subject contains the given string
/// **and** that was sent to a specific recipient address.
///
/// Retries every 2 seconds for up to `timeout` duration.
///
/// # Returns
///
/// `true` if a matching email was found within the timeout, `false` otherwise.
pub async fn wait_for_ses_email_to(
    to_email: &str,
    subject_contains: &str,
    timeout: Duration,
) -> bool {
    let start = tokio::time::Instant::now();
    let poll_interval = Duration::from_secs(2);

    loop {
        let emails = get_sent_emails().await;
        if let Some(matched) = emails.iter().find(|e| {
            e.subject.contains(subject_contains)
                && e.destination
                    .to_addresses
                    .iter()
                    .any(|addr| addr == to_email)
        }) {
            info!(
                subject = %matched.subject,
                to = ?matched.destination.to_addresses,
                "Found matching email in LocalStack SES."
            );
            return true;
        }

        if start.elapsed() >= timeout {
            warn!(
                to_email,
                subject_contains,
                elapsed_secs = start.elapsed().as_secs(),
                total_emails = emails.len(),
                "Timed out waiting for email."
            );
            return false;
        }

        debug!(
            to_email,
            subject_contains,
            total_emails = emails.len(),
            "No matching email yet, retrying..."
        );
        tokio::time::sleep(poll_interval).await;
    }
}
