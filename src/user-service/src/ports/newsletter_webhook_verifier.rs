use std::fmt;

/// A validated value carried from a newsletter-provider webhook into a use case.
///
/// Constructors intentionally preserve the supplied text exactly. They apply
/// only bounds and transport-safe character checks; they do not normalize email
/// addresses or provider identifiers.
macro_rules! bounded_webhook_value {
    ($name:ident, $max_bytes:expr, $allow_whitespace:expr) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Option<Self> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > $max_bytes
                    || value.chars().any(char::is_control)
                    || (!$allow_whitespace && value.chars().any(char::is_whitespace))
                {
                    return None;
                }
                Some(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&"[REDACTED]")
                    .finish()
            }
        }
    };
}

bounded_webhook_value!(NewsletterWebhookDeliveryId, 256, false);
bounded_webhook_value!(NewsletterWebhookProviderContactId, 256, false);
bounded_webhook_value!(NewsletterWebhookEmailAddress, 320, false);
bounded_webhook_value!(NewsletterWebhookMailingListId, 256, false);
bounded_webhook_value!(NewsletterWebhookEventName, 128, false);

/// One received HTTP header as bytes, before any text decoding or normalization.
/// Header values are sensitive and are always redacted from Debug output.
pub struct NewsletterWebhookHeader {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

impl NewsletterWebhookHeader {
    pub fn new(name: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl fmt::Debug for NewsletterWebhookHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewsletterWebhookHeader")
            .field("name", &"[REDACTED]")
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Inputs to the provider-neutral verifier port.
///
/// `headers` must contain the original selected request headers, and
/// `raw_body` must be the original received body bytes. `arrived_at_unix_seconds`
/// comes from the trusted HTTP runtime clock, not from the payload.
pub struct NewsletterWebhookVerificationRequest {
    pub headers: Vec<NewsletterWebhookHeader>,
    pub raw_body: Vec<u8>,
    pub signing_secret: String,
    pub arrived_at_unix_seconds: i64,
}

impl fmt::Debug for NewsletterWebhookVerificationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewsletterWebhookVerificationRequest")
            .field("headers", &self.headers)
            .field("raw_body", &"[REDACTED]")
            .field("signing_secret", &"[REDACTED]")
            .field("arrived_at_unix_seconds", &self.arrived_at_unix_seconds)
            .finish()
    }
}

/// SHA-256 evidence over the exact raw body bytes authenticated by the provider
/// verifier. Consumers can persist this with the delivery ID to detect reuse of
/// one delivery identity with different bytes; they must not hash reconstructed
/// JSON in its place.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NewsletterWebhookRawBodySha256([u8; 32]);

impl NewsletterWebhookRawBodySha256 {
    pub fn new(value: [u8; 32]) -> Self {
        Self(value)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for NewsletterWebhookRawBodySha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NewsletterWebhookRawBodySha256([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsletterWebhookEventKind {
    ContactUnsubscribed,
    ContactDeleted,
    EmailUnsubscribed,
    MailingListSubscribed,
    MailingListUnsubscribed,
    EmailResubscribed,
    EmailHardBounced,
    EmailSpamReported,
}

/// A supported signed event. This records facts only; it makes no consent
/// decision and does not bind the provider contact to an Aura Historia user.
pub struct VerifiedNewsletterWebhookEvent {
    pub delivery_id: NewsletterWebhookDeliveryId,
    pub provider_event_name: NewsletterWebhookEventName,
    pub kind: NewsletterWebhookEventKind,
    pub event_time_unix_seconds: i64,
    pub provider_contact_id: NewsletterWebhookProviderContactId,
    pub email: NewsletterWebhookEmailAddress,
    pub mailing_list_id: Option<NewsletterWebhookMailingListId>,
}

impl fmt::Debug for VerifiedNewsletterWebhookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedNewsletterWebhookEvent")
            .field("delivery_id", &"[REDACTED]")
            .field("provider_event_name", &self.provider_event_name)
            .field("kind", &self.kind)
            .field("event_time_unix_seconds", &self.event_time_unix_seconds)
            .field("provider_contact_id", &self.provider_contact_id)
            .field("email", &"[REDACTED]")
            .field("mailing_list_id", &self.mailing_list_id)
            .finish()
    }
}

/// A correctly signed event that is unrelated to newsletter consent handling.
pub struct IgnoredNewsletterWebhookEvent {
    pub delivery_id: NewsletterWebhookDeliveryId,
    pub provider_event_name: NewsletterWebhookEventName,
    pub event_time_unix_seconds: i64,
}

impl fmt::Debug for IgnoredNewsletterWebhookEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IgnoredNewsletterWebhookEvent")
            .field("delivery_id", &"[REDACTED]")
            .field("provider_event_name", &self.provider_event_name)
            .field("event_time_unix_seconds", &self.event_time_unix_seconds)
            .finish()
    }
}

/// Authenticated result envelope shared by supported and ignored events.
#[derive(Debug)]
pub struct NewsletterWebhookVerification {
    pub raw_body_sha256: NewsletterWebhookRawBodySha256,
    pub outcome: NewsletterWebhookVerificationOutcome,
}

#[derive(Debug)]
pub enum NewsletterWebhookVerificationOutcome {
    Verified(VerifiedNewsletterWebhookEvent),
    Ignored(IgnoredNewsletterWebhookEvent),
}

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum NewsletterWebhookVerificationError {
    #[error("newsletter webhook signing secret is not configured")]
    MissingSigningSecret,
    #[error("newsletter webhook signing secret is invalid")]
    InvalidSigningSecret,
    #[error("newsletter webhook headers exceed the supported limits")]
    HeadersTooLarge,
    #[error("newsletter webhook body exceeds the supported limit")]
    BodyTooLarge,
    #[error("newsletter webhook required headers are missing")]
    MissingRequiredHeader,
    #[error("newsletter webhook headers are ambiguous")]
    AmbiguousHeader,
    #[error("newsletter webhook header encoding is invalid")]
    InvalidHeaderEncoding,
    #[error("newsletter webhook signature header is malformed")]
    InvalidSignatureHeader,
    #[error("newsletter webhook timestamp is invalid or outside the allowed tolerance")]
    InvalidTimestamp,
    #[error("newsletter webhook signature is invalid")]
    InvalidSignature,
    #[error("newsletter webhook payload is malformed")]
    MalformedPayload,
    #[error("newsletter webhook payload schema is unsupported")]
    UnsupportedSchema,
    #[error("newsletter webhook event payload is malformed")]
    InvalidEvent,
    #[error("newsletter webhook verification could not complete")]
    CryptographicFailure,
}

pub trait NewsletterWebhookVerifier: Send + Sync {
    fn verify(
        &self,
        request: NewsletterWebhookVerificationRequest,
    ) -> Result<NewsletterWebhookVerification, NewsletterWebhookVerificationError>;
}
