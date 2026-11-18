//! Authentication of the Cognito triggers' requests to the host's internal endpoints.
//!
//! A trigger signs the exact body bytes with HMAC-SHA256 over `<label><unix seconds>:<body>`,
//! keyed with a shared random key, and sends the timestamp and `v1=<base64 signature>` in
//! [`TIMESTAMP_HEADER`] and [`SIGNATURE_HEADER`]. Each [`SignaturePurpose`] has its own label, so
//! a request signed for one endpoint never verifies at another although both share the key. The
//! host accepts a request only within [`TIMESTAMP_TOLERANCE_SECONDS`] of its own clock, so a
//! captured request cannot be replayed later; a replay inside the window repeats an idempotent
//! registration or a read.
use base64::Engine;
use openssl::{hash::MessageDigest, memcmp, pkey::PKey, sign::Signer};
use std::fmt;

pub const TIMESTAMP_HEADER: &str = "x-aura-signature-timestamp";
pub const SIGNATURE_HEADER: &str = "x-aura-signature";
/// How far the request's timestamp may lie from the receiver's clock, in either direction.
pub const TIMESTAMP_TOLERANCE_SECONDS: i64 = 300;
const SIGNATURE_VERSION_PREFIX: &str = "v1=";
const MIN_KEY_BYTES: usize = 32;
const HMAC_SHA256_BYTES: usize = 32;

/// The endpoint a signature is valid for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignaturePurpose {
    /// `POST /internal/cognito/post-confirmation`: register a confirmed user.
    PostConfirmation,
    /// `POST /internal/cognito/pre-sign-up`: resolve a federated sign-up's existing account.
    PreSignUp,
}

impl SignaturePurpose {
    /// Part of the wire protocol: the trigger and the host must agree on it.
    const fn label(self) -> &'static [u8] {
        match self {
            Self::PostConfirmation => b"aura-historia.cognito-post-confirmation.v1:",
            Self::PreSignUp => b"aura-historia.cognito-pre-sign-up.v1:",
        }
    }
}

/// The shared signing key. At least 32 bytes; never formatted or logged.
#[derive(Clone, PartialEq, Eq)]
pub struct SigningKey(Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the Cognito trigger signing key must hold at least {MIN_KEY_BYTES} bytes")]
pub struct WeakSigningKey;

impl SigningKey {
    pub fn new(key: impl Into<String>) -> Result<Self, WeakSigningKey> {
        let key = key.into();
        let key = key.trim();
        if key.len() < MIN_KEY_BYTES {
            return Err(WeakSigningKey);
        }
        Ok(Self(key.as_bytes().to_vec()))
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SigningKey(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("the request timestamp is missing or malformed")]
    MalformedTimestamp,
    #[error("the request timestamp is outside the accepted window")]
    StaleTimestamp,
    #[error("the request signature is missing or malformed")]
    MalformedSignature,
    #[error("the request signature does not match")]
    InvalidSignature,
    #[error("the request signature could not be computed")]
    CryptographicFailure,
}

/// The value of [`SIGNATURE_HEADER`] for `body` sent for `purpose` at `timestamp` (Unix seconds).
pub fn sign(
    key: &SigningKey,
    purpose: SignaturePurpose,
    timestamp: i64,
    body: &[u8],
) -> Result<String, SignatureError> {
    let signature = hmac_sha256(key, purpose, timestamp, body)?;
    Ok(format!(
        "{SIGNATURE_VERSION_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(signature)
    ))
}

/// Verifies the triggers' requests against the current key and, while it rotates, the previous one.
/// The comparison is constant-time, and the outcome does not reveal which key matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureVerifier {
    current: SigningKey,
    previous: Option<SigningKey>,
}

impl SignatureVerifier {
    pub fn new(current: SigningKey, previous: Option<SigningKey>) -> Self {
        Self { current, previous }
    }

    /// `purpose` is the receiving endpoint's; `timestamp` and `signature` are the raw header
    /// values; `now` is the receiver's clock in Unix seconds.
    pub fn verify(
        &self,
        purpose: SignaturePurpose,
        timestamp: Option<&[u8]>,
        signature: Option<&[u8]>,
        body: &[u8],
        now: i64,
    ) -> Result<(), SignatureError> {
        let timestamp = parse_timestamp(timestamp)?;
        if timestamp.abs_diff(now) > TIMESTAMP_TOLERANCE_SECONDS.unsigned_abs() {
            return Err(SignatureError::StaleTimestamp);
        }
        let candidate = parse_signature(signature)?;
        let mut matched = false;
        for key in std::iter::once(&self.current).chain(self.previous.as_ref()) {
            let expected = hmac_sha256(key, purpose, timestamp, body)?;
            matched |= memcmp::eq(&expected, &candidate);
        }
        if matched {
            Ok(())
        } else {
            Err(SignatureError::InvalidSignature)
        }
    }
}

fn parse_timestamp(value: Option<&[u8]>) -> Result<i64, SignatureError> {
    let value = value.ok_or(SignatureError::MalformedTimestamp)?;
    if value.is_empty() || value.len() > 12 || !value.iter().all(u8::is_ascii_digit) {
        return Err(SignatureError::MalformedTimestamp);
    }
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or(SignatureError::MalformedTimestamp)
}

fn parse_signature(value: Option<&[u8]>) -> Result<Vec<u8>, SignatureError> {
    let encoded = value
        .and_then(|value| value.strip_prefix(SIGNATURE_VERSION_PREFIX.as_bytes()))
        .ok_or(SignatureError::MalformedSignature)?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| SignatureError::MalformedSignature)?;
    if signature.len() != HMAC_SHA256_BYTES {
        return Err(SignatureError::MalformedSignature);
    }
    Ok(signature)
}

fn hmac_sha256(
    key: &SigningKey,
    purpose: SignaturePurpose,
    timestamp: i64,
    body: &[u8],
) -> Result<Vec<u8>, SignatureError> {
    let failure = |_| SignatureError::CryptographicFailure;
    let key = PKey::hmac(&key.0).map_err(failure)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(failure)?;
    signer.update(purpose.label()).map_err(failure)?;
    signer
        .update(timestamp.to_string().as_bytes())
        .map_err(failure)?;
    signer.update(b":").map_err(failure)?;
    signer.update(body).map_err(failure)?;
    signer.sign_to_vec().map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = br#"{"triggerSource":"PostConfirmation_ConfirmSignUp"}"#;
    const NOW: i64 = 1_800_000_000;
    const PURPOSE: SignaturePurpose = SignaturePurpose::PostConfirmation;

    fn key(value: &str) -> SigningKey {
        SigningKey::new(value.repeat(32)).unwrap()
    }

    fn verify(
        verifier: &SignatureVerifier,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<(), SignatureError> {
        verifier.verify(
            PURPOSE,
            Some(timestamp.as_bytes()),
            Some(signature.as_bytes()),
            body,
            NOW,
        )
    }

    #[test]
    fn should_accept_a_request_signed_with_the_current_or_previous_key() {
        let verifier = SignatureVerifier::new(key("c"), Some(key("p")));

        for signing in [key("c"), key("p")] {
            let signature = sign(&signing, PURPOSE, NOW, BODY).unwrap();
            assert_eq!(
                Ok(()),
                verify(&verifier, &NOW.to_string(), &signature, BODY)
            );
        }
    }

    #[test]
    fn should_reject_a_foreign_key_or_a_changed_body_or_timestamp() {
        let verifier = SignatureVerifier::new(key("c"), None);
        let signature = sign(&key("c"), PURPOSE, NOW, BODY).unwrap();

        assert_eq!(
            Err(SignatureError::InvalidSignature),
            verify(
                &verifier,
                &NOW.to_string(),
                &sign(&key("x"), PURPOSE, NOW, BODY).unwrap(),
                BODY
            )
        );
        assert_eq!(
            Err(SignatureError::InvalidSignature),
            verify(&verifier, &NOW.to_string(), &signature, b"{}")
        );
        assert_eq!(
            Err(SignatureError::InvalidSignature),
            verify(&verifier, &(NOW - 1).to_string(), &signature, BODY)
        );
    }

    #[test]
    fn should_reject_a_request_signed_for_another_purpose() {
        let verifier = SignatureVerifier::new(key("c"), None);
        let signature = sign(&key("c"), SignaturePurpose::PreSignUp, NOW, BODY).unwrap();

        assert_eq!(
            Err(SignatureError::InvalidSignature),
            verify(&verifier, &NOW.to_string(), &signature, BODY)
        );
        assert_eq!(
            Ok(()),
            verifier.verify(
                SignaturePurpose::PreSignUp,
                Some(NOW.to_string().as_bytes()),
                Some(signature.as_bytes()),
                BODY,
                NOW,
            )
        );
    }

    #[test]
    fn should_reject_timestamps_outside_the_window_in_either_direction() {
        let verifier = SignatureVerifier::new(key("c"), None);

        for timestamp in [
            NOW - TIMESTAMP_TOLERANCE_SECONDS,
            NOW + TIMESTAMP_TOLERANCE_SECONDS,
        ] {
            let signature = sign(&key("c"), PURPOSE, timestamp, BODY).unwrap();
            assert_eq!(
                Ok(()),
                verify(&verifier, &timestamp.to_string(), &signature, BODY)
            );
        }
        for timestamp in [
            NOW - TIMESTAMP_TOLERANCE_SECONDS - 1,
            NOW + TIMESTAMP_TOLERANCE_SECONDS + 1,
        ] {
            let signature = sign(&key("c"), PURPOSE, timestamp, BODY).unwrap();
            assert_eq!(
                Err(SignatureError::StaleTimestamp),
                verify(&verifier, &timestamp.to_string(), &signature, BODY)
            );
        }
    }

    #[test]
    fn should_reject_malformed_or_missing_headers() {
        let verifier = SignatureVerifier::new(key("c"), None);
        let signature = sign(&key("c"), PURPOSE, NOW, BODY).unwrap();

        for timestamp in ["", "-1", "1.5", " 1800000000", "18000000000000"] {
            assert_eq!(
                Err(SignatureError::MalformedTimestamp),
                verify(&verifier, timestamp, &signature, BODY)
            );
        }
        for malformed in [
            "",
            "v2=AAAA",
            "v1=not base64",
            "v1=AAAA",
            signature.trim_start_matches("v1="),
        ] {
            assert_eq!(
                Err(SignatureError::MalformedSignature),
                verify(&verifier, &NOW.to_string(), malformed, BODY)
            );
        }
        assert_eq!(
            Err(SignatureError::MalformedTimestamp),
            verifier.verify(PURPOSE, None, Some(signature.as_bytes()), BODY, NOW)
        );
        assert_eq!(
            Err(SignatureError::MalformedSignature),
            verifier.verify(PURPOSE, Some(NOW.to_string().as_bytes()), None, BODY, NOW)
        );
    }

    #[test]
    fn should_refuse_short_keys_and_never_format_a_key() {
        assert_eq!(Err(WeakSigningKey), SigningKey::new("k".repeat(31)));
        assert_eq!(
            Err(WeakSigningKey),
            SigningKey::new(format!(" {} ", "k".repeat(31)))
        );
        let secret = "s".repeat(32);
        let formatted = format!(
            "{:?}",
            SignatureVerifier::new(SigningKey::new(&*secret).unwrap(), None)
        );
        assert!(!formatted.contains(&secret));
    }
}
