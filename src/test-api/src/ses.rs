use crate::IntegrationTestService;
use crate::aws::{get_aws_config, get_endpoint_url};
use async_trait::async_trait;
use aws_sdk_sesv2::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

/// Sender identity verified for Floci SES integration tests.
const SENDER_EMAIL: &str = "no-reply@notify.aura-historia.com";

static SENDER_VERIFIED: OnceCell<()> = OnceCell::const_new();

/// Creates an owned Floci client on the calling runtime.
///
/// Clone it within that runtime to share its connection pool. Obtain another client after
/// the runtime stops, including during fixture cleanup.
pub async fn get_ses_client() -> Client {
    let client = Client::new(get_aws_config().await);
    debug!("Successfully initialized SESv2-Client.");
    client
}

/// Marker type representing the SES service in Floci-based tests.
///
/// Implements [`IntegrationTestService`] for use with the `#[aura_integration_test]` macro.
///
/// On [`set_up`](IntegrationTestService::set_up), verifies the sender email identity
/// (`no-reply@notify.aura-historia.com`) for Floci tests.
pub struct Ses();

#[async_trait]
impl IntegrationTestService for Ses {
    fn service_names(&self) -> &'static [&'static str] {
        &["sesv2"]
    }

    async fn set_up(&self) {
        if let Some(context) = crate::TestContext::current() {
            let sender = context.sender_email().expect("scoped SES sender");
            context
                .persist_resources()
                .await
                .expect("persist scoped SES sender");
            verify_sender_identity(&sender).await;
            return;
        }
        SENDER_VERIFIED
            .get_or_init(|| verify_sender_identity(SENDER_EMAIL))
            .await;
    }

    async fn tear_down(&self) {
        if crate::TestContext::current().is_some() {
            return;
        }
        clear_sent_emails().await;
    }
}

// ---------------------------------------------------------------------------
// SES identity verification
// ---------------------------------------------------------------------------

/// Verifies an email identity in Floci SES so that it can be used as a
/// sender address.
///
/// On real AWS this would trigger a verification email; Floci marks the
/// identity as verified immediately.
pub async fn verify_sender_identity(email: &str) {
    let client = get_ses_client().await;
    match client
        .create_email_identity()
        .email_identity(email)
        .send()
        .await
    {
        Ok(_) => {}
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|error| error.is_already_exists_exception()) => {}
        Err(error) => panic!("shouldn't fail creating SES email identity '{email}': {error}"),
    }
    let identity = client
        .get_email_identity()
        .email_identity(email)
        .send()
        .await
        .expect("shouldn't fail reading SES identity verification");
    assert!(
        identity.verified_for_sending_status(),
        "SES identity must be verified"
    );
    info!("Verified SES sender identity '{email}'.");
}

// ---------------------------------------------------------------------------
// Floci /_aws/ses introspection types
// ---------------------------------------------------------------------------

/// Response shape returned by Floci's `GET /_aws/ses` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct SentEmailsResponse {
    pub messages: Vec<SentEmail>,
}

/// A single email captured by Floci's in-memory SES store.
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

/// Retrieves all emails sent through Floci SES.
///
/// Queries the `/_aws/ses` internal endpoint which Floci exposes for
/// retrospecting sent messages.
pub async fn get_sent_emails() -> Vec<SentEmail> {
    if let Some(context) = crate::TestContext::current() {
        return get_sent_emails_from(&context.sender_email().expect("scoped SES sender")).await;
    }
    let url = format!("{}/_aws/ses", get_endpoint_url());
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail querying Floci SES endpoint: {e}"))
        .error_for_status()
        .expect("SES inspection endpoint must succeed");

    let body = response
        .json::<SentEmailsResponse>()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail deserializing SES messages response: {e}"));

    body.messages
}

/// Retrieves emails sent through Floci SES filtered by source email.
///
/// Uses the `email` query parameter on the `/_aws/ses` endpoint.
pub async fn get_sent_emails_from(source_email: &str) -> Vec<SentEmail> {
    let url = format!("{}/_aws/ses", get_endpoint_url());
    let response = reqwest::Client::new()
        .get(&url)
        .query(&[("email", source_email)])
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail querying Floci SES endpoint: {e}"))
        .error_for_status()
        .expect("SES inspection endpoint must succeed");

    let body = response
        .json::<SentEmailsResponse>()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail deserializing SES messages response: {e}"));

    body.messages
}

/// Deletes all sent emails from Floci's in-memory SES store.
///
/// Intended for test isolation — call this in `tear_down` so that each test
/// starts with a clean slate.
pub async fn clear_sent_emails() {
    if let Some(context) = crate::TestContext::current() {
        clear_source(&context.sender_email().expect("scoped SES sender"))
            .await
            .expect("clear owned SES captures");
        return;
    }
    assert!(
        crate::environment::managed_environment().is_none() || crate::context::exclusive_active(),
        "unscoped SES mailbox deletion is forbidden in managed mode"
    );
    let url = format!("{}/_aws/ses", get_endpoint_url());
    reqwest::Client::new()
        .delete(&url)
        .send()
        .await
        .unwrap_or_else(|e| panic!("shouldn't fail clearing Floci SES messages: {e}"))
        .error_for_status()
        .expect("SES inspection cleanup must succeed");
    debug!("Cleared all sent emails from Floci SES.");
}

/// Polls Floci SES for an email whose subject contains the given string.
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
                "Found matching email in Floci SES."
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

/// Polls Floci SES for an email whose subject contains the given string
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
                "Found matching email in Floci SES."
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

/// Floci 2.3.0 filters GET by source and DELETE by message ID, never by source.
pub(crate) async fn clear_source(source: &str) -> Result<(), application::error::BoxError> {
    #[derive(Deserialize)]
    struct Capture {
        #[serde(rename = "Id")]
        id: String,
        #[serde(rename = "Source")]
        source: String,
    }
    #[derive(Deserialize)]
    struct Captures {
        messages: Vec<Capture>,
    }
    let url = format!("{}/_aws/ses", get_endpoint_url());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let body = client
        .get(&url)
        .query(&[("email", source)])
        .send()
        .await?
        .error_for_status()?
        .json::<Captures>()
        .await?;
    for message in body.messages {
        if message.source != source {
            return Err(std::io::Error::other("SES returned an unowned capture").into());
        }
        client
            .delete(&url)
            .query(&[("id", message.id)])
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}
