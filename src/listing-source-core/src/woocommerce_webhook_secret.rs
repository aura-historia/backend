use std::fmt::{Debug, Display, Formatter};

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WoocommerceWebhookSecret(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("WooCommerce webhook secret must be nonblank")]
pub struct InvalidWoocommerceWebhookSecret;

impl WoocommerceWebhookSecret {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for WoocommerceWebhookSecret {
    type Error = InvalidWoocommerceWebhookSecret;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InvalidWoocommerceWebhookSecret);
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for WoocommerceWebhookSecret {
    type Error = InvalidWoocommerceWebhookSecret;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InvalidWoocommerceWebhookSecret);
        }
        Ok(Self(value))
    }
}

impl Debug for WoocommerceWebhookSecret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WoocommerceWebhookSecret([REDACTED])")
    }
}

impl Display for WoocommerceWebhookSecret {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::WoocommerceWebhookSecret;

    #[test]
    fn preserves_secret_exactly_and_redacts_formatting() {
        let value = "  secret\t";
        let secret = WoocommerceWebhookSecret::try_from(value).unwrap();

        assert_eq!(value, secret.as_str());
        assert_eq!("[REDACTED]", secret.to_string());
        assert!(!format!("{secret:?}").contains(value));
    }

    #[test]
    fn rejects_empty_and_unicode_whitespace_secrets() {
        assert!(WoocommerceWebhookSecret::try_from("").is_err());
        assert!(WoocommerceWebhookSecret::try_from("\u{2003}\t\n").is_err());
    }
}
