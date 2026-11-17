//! Authentication and parsing of Stripe webhook deliveries; the contract is "Stripe
//! subscription reconciliation" in `docs/events/flow.md`.

use openssl::{
    hash::{MessageDigest, hash},
    pkey::{PKey, Private},
    sign::Signer,
};
use serde::Deserialize;
use std::fmt;
use user_core::stripe_customer_id::StripeCustomerId;
use user_service::ports::stripe_subscription_sync::{
    StripeSubscriptionEvent, StripeWebhookEvent, StripeWebhookVerificationError,
    StripeWebhookVerifier,
};

/// The API version the webhook endpoint must be created with; events rendered with any
/// other version are rejected so a shape change cannot be parsed by accident.
pub const STRIPE_WEBHOOK_API_VERSION: &str = "2026-10-28.endive";

pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED: &str = "customer.subscription.created";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED: &str = "customer.subscription.updated";
pub const STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED: &str = "customer.subscription.deleted";

const SIGNATURE_TOLERANCE_SECONDS: u64 = 300;
const SIGNING_SECRET_PREFIX: &str = "whsec_";
const MAX_SIGNING_SECRET_BYTES: usize = 1024;
const MAX_SIGNATURE_HEADER_BYTES: usize = 4 * 1024;
const MAX_SIGNATURE_CANDIDATES: usize = 8;
const SIGNATURE_BYTES: usize = 32;

#[derive(Debug, thiserror::Error)]
#[error("invalid Stripe webhook signing secret")]
pub struct InvalidStripeWebhookSigningSecret;

/// Verifies `Stripe-Signature` with the endpoint signing secret.
pub struct StripeSubscriptionWebhookVerifier {
    key: PKey<Private>,
}

impl StripeSubscriptionWebhookVerifier {
    pub fn new(signing_secret: &str) -> Result<Self, InvalidStripeWebhookSigningSecret> {
        let valid = signing_secret
            .strip_prefix(SIGNING_SECRET_PREFIX)
            .is_some_and(|suffix| !suffix.is_empty())
            && signing_secret.len() <= MAX_SIGNING_SECRET_BYTES
            && signing_secret.bytes().all(|byte| byte.is_ascii_graphic());
        if !valid {
            return Err(InvalidStripeWebhookSigningSecret);
        }
        // Stripe keys the HMAC with the whole secret text, prefix included.
        let key =
            PKey::hmac(signing_secret.as_bytes()).map_err(|_| InvalidStripeWebhookSigningSecret)?;
        Ok(Self { key })
    }

    fn signature(
        &self,
        timestamp: &[u8],
        raw_body: &[u8],
    ) -> Result<Vec<u8>, StripeWebhookVerificationError> {
        let failure = |_| StripeWebhookVerificationError::CryptographicFailure;
        let mut signer = Signer::new(MessageDigest::sha256(), &self.key).map_err(failure)?;
        signer.update(timestamp).map_err(failure)?;
        signer.update(b".").map_err(failure)?;
        signer.update(raw_body).map_err(failure)?;
        signer.sign_to_vec().map_err(failure)
    }
}

impl fmt::Debug for StripeSubscriptionWebhookVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StripeSubscriptionWebhookVerifier")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl StripeWebhookVerifier for StripeSubscriptionWebhookVerifier {
    fn verify(
        &self,
        signature_header: &[u8],
        raw_body: &[u8],
        arrived_at_unix_seconds: i64,
    ) -> Result<StripeWebhookEvent, StripeWebhookVerificationError> {
        let header = SignatureHeader::parse(signature_header)?;
        if header.timestamp.abs_diff(arrived_at_unix_seconds) > SIGNATURE_TOLERANCE_SECONDS {
            return Err(StripeWebhookVerificationError::InvalidSignature);
        }
        let expected = self.signature(header.timestamp_text, raw_body)?;
        let verified = header.signatures.iter().fold(false, |matched, candidate| {
            // Compare every candidate, even after one matches.
            openssl::memcmp::eq(&expected, candidate) | matched
        });
        if !verified {
            return Err(StripeWebhookVerificationError::InvalidSignature);
        }
        parse_event(raw_body)
    }
}

/// `t=<unix seconds>,v1=<hex>[,v1=<hex>…]`; one `v1` per active signing secret, so a
/// rotating endpoint verifies against either secret. Stripe may add other entries in any
/// order, such as the `v0` test scheme or unsigned debug metadata; only `t` and `v1` count.
struct SignatureHeader<'a> {
    timestamp: i64,
    timestamp_text: &'a [u8],
    signatures: Vec<[u8; SIGNATURE_BYTES]>,
}

impl<'a> SignatureHeader<'a> {
    fn parse(value: &'a [u8]) -> Result<Self, StripeWebhookVerificationError> {
        let invalid = StripeWebhookVerificationError::InvalidSignature;
        if value.len() > MAX_SIGNATURE_HEADER_BYTES {
            return Err(invalid);
        }
        let mut timestamp_text = None;
        let mut signatures = Vec::new();
        for element in value.split(|byte| *byte == b',') {
            let Some(separator) = element.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let (key, value) = (&element[..separator], &element[separator + 1..]);
            match key {
                b"t" if timestamp_text.is_none() => timestamp_text = Some(value),
                b"t" => return Err(invalid),
                b"v1" if signatures.len() < MAX_SIGNATURE_CANDIDATES => {
                    signatures.push(decode_hex_signature(value).ok_or(invalid)?);
                }
                b"v1" => return Err(invalid),
                _ => {}
            }
        }
        let timestamp_text = timestamp_text.ok_or(invalid)?;
        if signatures.is_empty()
            || timestamp_text.is_empty()
            || !timestamp_text.iter().all(u8::is_ascii_digit)
        {
            return Err(invalid);
        }
        let timestamp = std::str::from_utf8(timestamp_text)
            .ok()
            .and_then(|text| text.parse::<i64>().ok())
            .ok_or(invalid)?;
        Ok(Self {
            timestamp,
            timestamp_text,
            signatures,
        })
    }
}

fn decode_hex_signature(value: &[u8]) -> Option<[u8; SIGNATURE_BYTES]> {
    if value.len() != SIGNATURE_BYTES * 2 {
        return None;
    }
    let mut decoded = [0; SIGNATURE_BYTES];
    for (byte, [high, low]) in decoded.iter_mut().zip(value.as_chunks::<2>().0) {
        *byte = (hex_digit(*high)? << 4) | hex_digit(*low)?;
    }
    Some(decoded)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[derive(Deserialize)]
struct EventEnvelope {
    object: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct SubscriptionEvent {
    id: String,
    api_version: Option<String>,
    data: SubscriptionEventData,
}

#[derive(Deserialize)]
struct SubscriptionEventData {
    object: Subscription,
}

#[derive(Deserialize)]
struct Subscription {
    id: String,
    object: String,
    customer: Customer,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Customer {
    Id(String),
    Expanded { id: String },
}

fn parse_event(raw_body: &[u8]) -> Result<StripeWebhookEvent, StripeWebhookVerificationError> {
    let malformed = |_| StripeWebhookVerificationError::MalformedEvent;
    let envelope: EventEnvelope = serde_json::from_slice(raw_body).map_err(malformed)?;
    if envelope.object != "event" {
        return Err(StripeWebhookVerificationError::MalformedEvent);
    }
    if !matches!(
        envelope.kind.as_str(),
        STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED
            | STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED
            | STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED
    ) {
        return Ok(StripeWebhookEvent::Ignored);
    }
    let event: SubscriptionEvent = serde_json::from_slice(raw_body).map_err(malformed)?;
    if event.api_version.as_deref() != Some(STRIPE_WEBHOOK_API_VERSION)
        || event.data.object.object != "subscription"
    {
        return Err(StripeWebhookVerificationError::MalformedEvent);
    }
    let customer = match event.data.object.customer {
        Customer::Id(id) | Customer::Expanded { id } => id,
    };
    let subscription_id = event.data.object.id;
    let fingerprint = fingerprint(&envelope.kind, &subscription_id, &customer)?;
    Ok(StripeWebhookEvent::Subscription(StripeSubscriptionEvent {
        event_id: event.id,
        subscription_id,
        customer_id: StripeCustomerId::from(customer),
        fingerprint,
    }))
}

/// Covers what the event asks for, not its rendering: delivery metadata such as
/// `pending_webhooks` changes between deliveries of one event, and the snapshot depends on
/// the destination's API version.
fn fingerprint(
    kind: &str,
    subscription_id: &str,
    customer_id: &str,
) -> Result<[u8; 32], StripeWebhookVerificationError> {
    let content = [kind, subscription_id, customer_id].join("\n");
    hash(MessageDigest::sha256(), content.as_bytes())
        .map_err(|_| StripeWebhookVerificationError::CryptographicFailure)?
        .as_ref()
        .try_into()
        .map_err(|_| StripeWebhookVerificationError::CryptographicFailure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SECRET: &str = "whsec_test_current";
    const ROTATED_SECRET: &str = "whsec_test_previous";
    const NOW: i64 = 1_800_000_000;

    fn verifier() -> StripeSubscriptionWebhookVerifier {
        StripeSubscriptionWebhookVerifier::new(SECRET).expect("valid secret")
    }

    fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
        let signature = StripeSubscriptionWebhookVerifier::new(secret)
            .expect("valid secret")
            .signature(timestamp.to_string().as_bytes(), body)
            .expect("signature");
        signature.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn header(timestamp: i64, signatures: &[String]) -> Vec<u8> {
        let mut header = format!("t={timestamp}");
        for signature in signatures {
            header.push_str(",v1=");
            header.push_str(signature);
        }
        header.into_bytes()
    }

    fn event_body(kind: &str, extra: serde_json::Value) -> Vec<u8> {
        let mut event = json!({
            "id": "evt_1",
            "object": "event",
            "api_version": STRIPE_WEBHOOK_API_VERSION,
            "type": kind,
            "pending_webhooks": 1,
            "data": {"object": {"id": "sub_1", "object": "subscription", "customer": "cus_1"}},
        });
        if let (Some(event), Some(extra)) = (event.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                event.insert(key.clone(), value.clone());
            }
        }
        serde_json::to_vec(&event).expect("event JSON")
    }

    fn verify_signed(body: &[u8]) -> Result<StripeWebhookEvent, StripeWebhookVerificationError> {
        verifier().verify(&header(NOW, &[sign(SECRET, NOW, body)]), body, NOW)
    }

    fn subscription(event: StripeWebhookEvent) -> StripeSubscriptionEvent {
        match event {
            StripeWebhookEvent::Subscription(event) => event,
            StripeWebhookEvent::Ignored => panic!("expected a subscription event"),
        }
    }

    mod signature {
        use super::*;

        #[test]
        fn should_accept_a_valid_signature() {
            let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({}));

            let event = subscription(verify_signed(&body).expect("verified"));

            assert_eq!("evt_1", event.event_id);
            assert_eq!("sub_1", event.subscription_id);
            assert_eq!("cus_1", event.customer_id.as_ref());
        }

        #[test]
        fn should_accept_a_signature_computed_independently_over_timestamp_and_raw_body() {
            // HMAC-SHA256("whsec_test_current", "1800000000." + body), computed outside this crate.
            let body = br#"{"id":"evt_1","object":"event","type":"invoice.paid"}"#;
            let header =
                b"t=1800000000,v1=aa5f3725c8f7ae33165758faa1d706d0624ac4da4eda46601abd60097fc2179c";

            assert_eq!(
                Ok(StripeWebhookEvent::Ignored),
                verifier().verify(header, body, NOW)
            );
        }

        #[test]
        fn should_reject_a_signature_made_with_another_secret() {
            let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({}));

            let result = verifier().verify(
                &header(NOW, &[sign(ROTATED_SECRET, NOW, &body)]),
                &body,
                NOW,
            );

            assert_eq!(
                Err(StripeWebhookVerificationError::InvalidSignature),
                result
            );
        }

        #[test]
        fn should_reject_a_timestamp_outside_the_five_minute_tolerance() {
            let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({}));
            for (timestamp, accepted) in [
                (NOW - 300, true),
                (NOW + 300, true),
                (NOW - 301, false),
                (NOW + 301, false),
            ] {
                let result = verifier().verify(
                    &header(timestamp, &[sign(SECRET, timestamp, &body)]),
                    &body,
                    NOW,
                );

                assert_eq!(
                    accepted,
                    result.is_ok(),
                    "timestamp offset {}",
                    timestamp - NOW
                );
            }
        }

        #[test]
        fn should_accept_any_matching_v1_signature_when_several_are_present() {
            let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({}));
            let rotating = header(
                NOW,
                &[sign(ROTATED_SECRET, NOW, &body), sign(SECRET, NOW, &body)],
            );
            let mut with_unsigned_entries = b"secret=whsec_****Vt,".to_vec();
            with_unsigned_entries.extend_from_slice(&rotating);
            with_unsigned_entries
                .extend_from_slice(b",v0=ignored,destination=we_1,payload=a25aefc6,unknown");

            assert!(verifier().verify(&rotating, &body, NOW).is_ok());
            assert!(
                verifier()
                    .verify(&with_unsigned_entries, &body, NOW)
                    .is_ok()
            );
        }

        #[test]
        fn should_reject_a_tampered_body() {
            let body = event_body(STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED, json!({}));
            let tampered = event_body(
                STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
                json!({"data": {"object": {"id": "sub_1", "object": "subscription", "customer": "cus_2"}}}),
            );

            let result =
                verifier().verify(&header(NOW, &[sign(SECRET, NOW, &body)]), &tampered, NOW);

            assert_eq!(
                Err(StripeWebhookVerificationError::InvalidSignature),
                result
            );
        }

        #[test]
        fn should_reject_malformed_signature_headers_before_parsing_the_body() {
            let body = b"not json";
            let signature = sign(SECRET, NOW, body);
            for value in [
                String::new(),
                format!("v1={signature}"),
                format!("t={NOW}"),
                format!("t={NOW},t={NOW},v1={signature}"),
                format!("t=-{NOW},v1={signature}"),
                format!("t={NOW},v1={}", signature.to_uppercase()),
                format!("t={NOW},v1={}", &signature[2..]),
                format!("t={NOW}{}", format!(",v1={signature}").repeat(9)),
            ] {
                assert_eq!(
                    Err(StripeWebhookVerificationError::InvalidSignature),
                    verifier().verify(value.as_bytes(), body, NOW),
                    "{value}"
                );
            }
        }

        #[test]
        fn should_reject_signing_secrets_without_the_stripe_prefix() {
            for secret in ["", "whsec_", "secret", "whsec_ spaced"] {
                assert!(StripeSubscriptionWebhookVerifier::new(secret).is_err());
            }
        }
    }

    mod parsing {
        use super::*;

        #[test]
        fn should_ignore_other_event_types() {
            let body = event_body("invoice.paid", json!({"data": {"object": {}}}));

            assert_eq!(Ok(StripeWebhookEvent::Ignored), verify_signed(&body));
        }

        #[test]
        fn should_accept_an_expanded_customer() {
            let body = event_body(
                STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
                json!({"data": {"object": {"id": "sub_1", "object": "subscription", "customer": {"id": "cus_1"}}}}),
            );

            let event = subscription(verify_signed(&body).expect("verified"));

            assert_eq!("cus_1", event.customer_id.as_ref());
        }

        #[test]
        fn should_reject_authentic_bodies_that_are_not_supported_events() {
            for body in [
                b"{".to_vec(),
                b"[]".to_vec(),
                event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
                    json!({"object": "list"}),
                ),
                event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
                    json!({"api_version": "2020-08-27"}),
                ),
                event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_CREATED,
                    json!({"api_version": null}),
                ),
                event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                    json!({"data": {"object": {}}}),
                ),
                event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                    json!({"data": {"object": {"id": "in_1", "object": "invoice", "customer": "cus_1"}}}),
                ),
            ] {
                assert_eq!(
                    Err(StripeWebhookVerificationError::MalformedEvent),
                    verify_signed(&body)
                );
            }
        }

        #[test]
        fn should_fingerprint_the_request_and_not_delivery_metadata() {
            let first = subscription(
                verify_signed(&event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                    json!({}),
                ))
                .expect("verified"),
            );
            let redelivered = subscription(
                verify_signed(&event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_UPDATED,
                    json!({"pending_webhooks": 0}),
                ))
                .expect("verified"),
            );
            let reused = subscription(
                verify_signed(&event_body(
                    STRIPE_EVENT_TYPE_SUBSCRIPTION_DELETED,
                    json!({}),
                ))
                .expect("verified"),
            );

            assert_eq!(first.fingerprint, redelivered.fingerprint);
            assert_ne!(first.fingerprint, reused.fingerprint);
        }
    }
}
