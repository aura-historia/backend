use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Verifies the Lemon Squeezy webhook signature.
///
/// Lemon Squeezy signs webhook payloads with HMAC-SHA256 using the webhook
/// secret and sends the hex-encoded digest in the `X-Signature` header.
pub fn verify_signature(payload: &str, signature: &str, secret: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    Mac::update(&mut mac, payload.as_bytes());

    let Ok(expected) = hex::decode(signature) else {
        return false;
    };

    mac.verify_slice(&expected).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_verify_valid_signature() {
        let secret = "test-webhook-secret";
        let payload = r#"{"meta":{"event_name":"subscription_created"}}"#;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        Mac::update(&mut mac, payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        assert!(verify_signature(payload, &signature, secret));
    }

    #[test]
    fn should_reject_invalid_signature() {
        let secret = "test-webhook-secret";
        let payload = r#"{"meta":{"event_name":"subscription_created"}}"#;
        let bad_signature = "deadbeefcafebabe0000000000000000000000000000000000000000000000ff";

        assert!(!verify_signature(payload, bad_signature, secret));
    }

    #[test]
    fn should_reject_non_hex_signature() {
        let secret = "test-webhook-secret";
        let payload = r#"{"meta":{"event_name":"subscription_created"}}"#;

        assert!(!verify_signature(payload, "not-hex-at-all!!!", secret));
    }

    #[test]
    fn should_reject_when_payload_tampered() {
        let secret = "test-webhook-secret";
        let original = r#"{"meta":{"event_name":"subscription_created"}}"#;
        let tampered = r#"{"meta":{"event_name":"subscription_updated"}}"#;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        Mac::update(&mut mac, original.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        assert!(!verify_signature(tampered, &signature, secret));
    }

    #[test]
    fn should_reject_when_wrong_secret() {
        let payload = r#"{"meta":{"event_name":"subscription_created"}}"#;

        let mut mac = HmacSha256::new_from_slice(b"correct-secret").unwrap();
        Mac::update(&mut mac, payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        assert!(!verify_signature(payload, &signature, "wrong-secret"));
    }
}
