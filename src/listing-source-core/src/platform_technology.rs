use std::fmt::{Display, Formatter};

/// An open platform code in SCREAMING_SNAKE_CASE, independent of ingestion methods.
///
/// Codes start with an ASCII uppercase letter and contain uppercase letters or
/// digits, with single underscores between words. Values are preserved exactly.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlatformTechnology(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformTechnologyError {
    #[error("platform technology must not be blank")]
    Blank,
    #[error("platform technology must not exceed {max_bytes} ASCII bytes (got {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error(
        "platform technology must be an ASCII SCREAMING_SNAKE_CASE identifier starting with a letter"
    )]
    InvalidIdentifier,
}

impl PlatformTechnology {
    pub const MAX_BYTES: usize = 64;
}

impl TryFrom<&str> for PlatformTechnology {
    type Error = PlatformTechnologyError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(PlatformTechnologyError::Blank);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(PlatformTechnologyError::TooLong {
                max_bytes: Self::MAX_BYTES,
                actual_bytes: value.len(),
            });
        }
        if !value.as_bytes()[0].is_ascii_uppercase()
            || !value.split('_').all(|word| {
                !word.is_empty()
                    && word
                        .bytes()
                        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
            })
        {
            return Err(PlatformTechnologyError::InvalidIdentifier);
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for PlatformTechnology {
    type Error = PlatformTechnologyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for PlatformTechnology {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for PlatformTechnology {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_known_and_new_canonical_platform_codes() {
        for code in [
            "SHOPIFY",
            "WOOCOMMERCE",
            "BIGCOMMERCE",
            "SHOPWARE",
            "NEW_PLATFORM_2",
            "A",
        ] {
            let platform = PlatformTechnology::try_from(code.to_owned())
                .unwrap_or_else(|error| panic!("valid platform code: {error}"));
            assert_eq!(code, platform.as_ref());
            assert_eq!(code, platform.to_string());
        }
    }

    #[test]
    fn should_reject_empty_platform_code() {
        assert_eq!(
            Err(PlatformTechnologyError::Blank),
            PlatformTechnology::try_from("")
        );
    }

    #[test]
    fn should_reject_noncanonical_platform_codes_without_normalizing() {
        for code in [
            "shopify",
            "Shopify",
            " SHOPIFY",
            "SHOPIFY ",
            "SHOPIFY-PLUS",
            "A B",
            "A\n",
            "_A",
            "A_",
            "A__B",
            "2CART",
            "SHÖP",
            "Ａ",
            "A\0",
        ] {
            assert_eq!(
                Err(PlatformTechnologyError::InvalidIdentifier),
                PlatformTechnology::try_from(code),
                "{code:?}",
            );
        }
    }

    #[test]
    fn should_enforce_platform_code_length_without_truncating() {
        let code = "A".repeat(PlatformTechnology::MAX_BYTES);
        assert_eq!(
            Ok(code.clone()),
            PlatformTechnology::try_from(code).map(|code| code.to_string())
        );
        assert_eq!(
            Err(PlatformTechnologyError::TooLong {
                max_bytes: 64,
                actual_bytes: 65
            }),
            PlatformTechnology::try_from("A".repeat(65)),
        );
    }
}
