use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::fmt;

pub const NEWSLETTER_CONFIRMATION_TOKEN_MAX_ENCODED_LENGTH: usize = 512;
const TOKEN_BYTES: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NewsletterConfirmationTokenDigest([u8; 32]);

impl NewsletterConfirmationTokenDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, InvalidNewsletterConfirmationToken> {
        let digest: [u8; 32] = bytes
            .try_into()
            .map_err(|_| InvalidNewsletterConfirmationToken)?;
        Ok(Self(digest))
    }
}

impl fmt::Debug for NewsletterConfirmationTokenDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NewsletterConfirmationTokenDigest([REDACTED])")
    }
}

/// A transient URL-safe bearer proof. Its contents are never displayed or serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct RawNewsletterConfirmationToken(String);

impl RawNewsletterConfirmationToken {
    /// Construct from exactly 32 bytes supplied by a cryptographically secure generator.
    pub fn from_entropy(bytes: [u8; TOKEN_BYTES]) -> Self {
        Self(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digest(&self) -> NewsletterConfirmationTokenDigest {
        NewsletterConfirmationTokenDigest(Sha256::digest(self.0.as_bytes()).into())
    }
}

impl TryFrom<&str> for RawNewsletterConfirmationToken {
    type Error = InvalidNewsletterConfirmationToken;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > NEWSLETTER_CONFIRMATION_TOKEN_MAX_ENCODED_LENGTH {
            return Err(InvalidNewsletterConfirmationToken);
        }
        let decoded = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| InvalidNewsletterConfirmationToken)?;
        if decoded.len() != TOKEN_BYTES || URL_SAFE_NO_PAD.encode(&decoded) != value {
            return Err(InvalidNewsletterConfirmationToken);
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Debug for RawNewsletterConfirmationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RawNewsletterConfirmationToken([REDACTED])")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid newsletter confirmation token")]
pub struct InvalidNewsletterConfirmationToken;

impl fmt::Debug for InvalidNewsletterConfirmationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvalidNewsletterConfirmationToken")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InvalidNewsletterConfirmationToken, NEWSLETTER_CONFIRMATION_TOKEN_MAX_ENCODED_LENGTH,
        RawNewsletterConfirmationToken,
    };

    #[test]
    fn encodes_32_random_bytes_as_canonical_url_safe_token_and_hashes_it() {
        let token = RawNewsletterConfirmationToken::from_entropy([0x5a; 32]);
        assert_eq!(43, token.as_str().len());
        assert!(
            token
                .as_str()
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' })
        );
        assert_eq!(
            token,
            RawNewsletterConfirmationToken::try_from(token.as_str()).unwrap()
        );
        assert_eq!(32, token.digest().as_bytes().len());
        assert!(!format!("{token:?}").contains(token.as_str()));
    }

    #[test]
    fn rejects_malformed_or_noncanonical_tokens_without_echoing_them() {
        for value in [
            "",
            "not-a-token",
            &"a".repeat(NEWSLETTER_CONFIRMATION_TOKEN_MAX_ENCODED_LENGTH + 1),
        ] {
            let error = RawNewsletterConfirmationToken::try_from(value)
                .expect_err("malformed input must fail");
            assert_eq!(InvalidNewsletterConfirmationToken, error);
            assert!(value.is_empty() || !error.to_string().contains(value));
        }
    }

    #[test]
    fn digest_debug_is_redacted() {
        let token = RawNewsletterConfirmationToken::from_entropy([0xa5; 32]);
        assert!(format!("{:?}", token.digest()).contains("REDACTED"));
    }
}
