use std::{fmt::Display, ops::Deref};

/// Canonical Party name, normalized by trimming Unicode whitespace at both ends.
///
/// Names must be nonblank and contain at most 255 UTF-8 bytes. The limit matches
/// the authoritative PostgreSQL constraint and values are never truncated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartyName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartyNameError {
    #[error("party name must not be blank")]
    Blank,
    #[error("party name must not exceed {max_bytes} UTF-8 bytes (got {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

impl PartyName {
    pub const MAX_BYTES: usize = 255;
}

impl TryFrom<&str> for PartyName {
    type Error = PartyNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(PartyNameError::Blank);
        }

        let actual_bytes = value.len();
        if actual_bytes > Self::MAX_BYTES {
            return Err(PartyNameError::TooLong {
                max_bytes: Self::MAX_BYTES,
                actual_bytes,
            });
        }

        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for PartyName {
    type Error = PartyNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl Display for PartyName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PartyName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PartyName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trim_unicode_outer_whitespace() {
        let name = PartyName::try_from("\u{2003} Antik und Stil \u{00a0}");

        assert_eq!(
            Ok("Antik und Stil".to_owned()),
            name.map(|value| value.to_string())
        );
    }

    #[test]
    fn should_reject_blank_name_after_unicode_trim() {
        assert_eq!(
            Err(PartyNameError::Blank),
            PartyName::try_from("\u{2003}\u{00a0}")
        );
    }

    #[test]
    fn should_reject_name_over_byte_cap_without_truncating() {
        let value = "é".repeat(128);

        assert_eq!(
            Err(PartyNameError::TooLong {
                max_bytes: PartyName::MAX_BYTES,
                actual_bytes: 256,
            }),
            PartyName::try_from(value.as_str())
        );
    }

    #[test]
    fn should_accept_name_at_byte_cap_without_truncating() {
        let value = format!("{}a", "é".repeat(127));
        let name = PartyName::try_from(value.as_str());

        assert_eq!(Ok(value), name.map(|value| value.to_string()));
    }
}
