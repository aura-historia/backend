//! Shopify webhook authentication: a base64 HMAC-SHA256 of the exact body bytes, keyed with the
//! app's client secret. A previous secret is accepted alongside the current one while it rotates.
use base64::Engine;
use openssl::{hash::MessageDigest, memcmp, pkey::PKey, sign::Signer};
use product_listing_service::ports::shopify_webhook_verifier::{
    ShopifyWebhookVerificationError, ShopifyWebhookVerifier,
};
use std::fmt;

const HMAC_SHA256_BYTES: usize = 32;

/// The app client secrets that may sign a delivery. Never formatted or logged.
#[derive(Clone, PartialEq, Eq)]
pub struct ShopifyWebhookSecrets {
    current: String,
    previous: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Shopify client secret must not be blank")]
pub struct BlankShopifyClientSecret;

impl ShopifyWebhookSecrets {
    pub fn new(
        current: String,
        previous: Option<String>,
    ) -> Result<Self, BlankShopifyClientSecret> {
        if current.trim().is_empty() || previous.as_deref().is_some_and(|p| p.trim().is_empty()) {
            return Err(BlankShopifyClientSecret);
        }
        Ok(Self { current, previous })
    }
}

impl fmt::Debug for ShopifyWebhookSecrets {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShopifyWebhookSecrets")
            .field("current", &"<redacted>")
            .field("previous", &self.previous.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Verifies `X-Shopify-Hmac-Sha256` against every configured secret with a constant-time
/// comparison, so the outcome does not reveal which secret matched.
#[derive(Debug, Clone)]
pub struct ShopifyHmacWebhookVerifier {
    secrets: ShopifyWebhookSecrets,
}

impl ShopifyHmacWebhookVerifier {
    pub fn new(secrets: ShopifyWebhookSecrets) -> Self {
        Self { secrets }
    }
}

impl ShopifyWebhookVerifier for ShopifyHmacWebhookVerifier {
    fn verify(
        &self,
        signatures: &[&[u8]],
        raw_body: &[u8],
    ) -> Result<(), ShopifyWebhookVerificationError> {
        let [signature] = signatures else {
            return Err(ShopifyWebhookVerificationError::MalformedSignature);
        };
        let candidate = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .map_err(|_| ShopifyWebhookVerificationError::MalformedSignature)?;
        if candidate.len() != HMAC_SHA256_BYTES {
            return Err(ShopifyWebhookVerificationError::MalformedSignature);
        }
        let secrets = &self.secrets;
        let mut matched = false;
        for secret in std::iter::once(&secrets.current).chain(secrets.previous.as_ref()) {
            let expected = hmac_sha256(secret.as_bytes(), raw_body)?;
            matched |= memcmp::eq(&expected, &candidate);
        }
        if matched {
            Ok(())
        } else {
            Err(ShopifyWebhookVerificationError::InvalidSignature)
        }
    }
}

fn hmac_sha256(key: &[u8], raw_body: &[u8]) -> Result<Vec<u8>, ShopifyWebhookVerificationError> {
    let failure = |_| ShopifyWebhookVerificationError::CryptographicFailure;
    let key = PKey::hmac(key).map_err(failure)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(failure)?;
    signer.update(raw_body).map_err(failure)?;
    signer.sign_to_vec().map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = br#"{"product_listing":{"product_id":42}}"#;

    fn signature(secret: &str, body: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(hmac_sha256(secret.as_bytes(), body).unwrap_or_else(|error| panic!("{error}")))
    }

    fn verifier(current: &str, previous: Option<&str>) -> ShopifyHmacWebhookVerifier {
        ShopifyHmacWebhookVerifier::new(
            ShopifyWebhookSecrets::new(current.to_owned(), previous.map(str::to_owned))
                .unwrap_or_else(|error| panic!("{error}")),
        )
    }

    fn verify(
        verifier: &ShopifyHmacWebhookVerifier,
        header: &str,
        body: &[u8],
    ) -> Result<(), ShopifyWebhookVerificationError> {
        verifier.verify(&[header.as_bytes()], body)
    }

    #[test]
    fn should_accept_signature_from_current_secret() {
        let verifier = verifier("current-secret", None);

        assert_eq!(
            Ok(()),
            verify(&verifier, &signature("current-secret", BODY), BODY)
        );
    }

    #[test]
    fn should_accept_signature_from_previous_secret_during_rotation() {
        let verifier = verifier("current-secret", Some("previous-secret"));

        assert_eq!(
            Ok(()),
            verify(&verifier, &signature("previous-secret", BODY), BODY)
        );
        assert_eq!(
            Ok(()),
            verify(&verifier, &signature("current-secret", BODY), BODY)
        );
    }

    #[test]
    fn should_reject_signature_from_wrong_secret() {
        let verifier = verifier("current-secret", Some("previous-secret"));

        assert_eq!(
            Err(ShopifyWebhookVerificationError::InvalidSignature),
            verify(&verifier, &signature("other-secret", BODY), BODY)
        );
    }

    #[test]
    fn should_reject_tampered_body() {
        let verifier = verifier("current-secret", None);
        let tampered = br#"{"product_listing":{"product_id":43}}"#;

        assert_eq!(
            Err(ShopifyWebhookVerificationError::InvalidSignature),
            verify(&verifier, &signature("current-secret", BODY), tampered)
        );
    }

    #[test]
    fn should_reject_missing_repeated_or_malformed_header() {
        let verifier = verifier("current-secret", None);
        let valid = signature("current-secret", BODY);

        assert_eq!(
            Err(ShopifyWebhookVerificationError::MalformedSignature),
            verifier.verify(&[], BODY)
        );
        assert_eq!(
            Err(ShopifyWebhookVerificationError::MalformedSignature),
            verifier.verify(&[valid.as_bytes(), valid.as_bytes()], BODY)
        );
        for malformed in ["not-base64!", "c2hvcnQ=", ""] {
            assert_eq!(
                Err(ShopifyWebhookVerificationError::MalformedSignature),
                verify(&verifier, malformed, BODY),
                "{malformed}"
            );
        }
    }

    #[test]
    fn should_reject_blank_secrets_and_redact_debug_output() {
        assert_eq!(
            Err(BlankShopifyClientSecret),
            ShopifyWebhookSecrets::new(" ".to_owned(), None)
        );
        assert_eq!(
            Err(BlankShopifyClientSecret),
            ShopifyWebhookSecrets::new("current".to_owned(), Some(String::new()))
        );

        let debug = format!("{:?}", verifier("current-secret", Some("previous-secret")));
        assert!(!debug.contains("current-secret") && !debug.contains("previous-secret"));
    }
}
