use std::{
    fmt::{Display, Formatter},
    ops::Deref,
};

/// Canonical ListingSource name, normalized by trimming Unicode whitespace at both ends.
///
/// Names must be nonblank and contain at most 255 UTF-8 bytes. The limit matches
/// the authoritative PostgreSQL constraint and values are never truncated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListingSourceNameError {
    #[error("listing source name must not be blank")]
    Blank,
    #[error("listing source name must not exceed {max_bytes} UTF-8 bytes (got {actual_bytes})")]
    TooLong {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

impl ListingSourceName {
    pub const MAX_BYTES: usize = 255;
}

impl TryFrom<&str> for ListingSourceName {
    type Error = ListingSourceNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ListingSourceNameError::Blank);
        }

        let actual_bytes = value.len();
        if actual_bytes > Self::MAX_BYTES {
            return Err(ListingSourceNameError::TooLong {
                max_bytes: Self::MAX_BYTES,
                actual_bytes,
            });
        }

        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for ListingSourceName {
    type Error = ListingSourceNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for ListingSourceName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for ListingSourceName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for ListingSourceName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trim_unicode_outer_whitespace_from_listing_source_name() {
        let name = ListingSourceName::try_from("\u{2003} Antik und Stil \u{00a0}");

        assert_eq!(
            Ok("Antik und Stil".to_owned()),
            name.map(|value| value.to_string())
        );
    }

    #[test]
    fn should_reject_blank_listing_source_name_after_unicode_trim() {
        assert_eq!(
            Err(ListingSourceNameError::Blank),
            ListingSourceName::try_from("\u{2003}\u{00a0}")
        );
    }

    #[test]
    fn should_reject_listing_source_name_over_byte_cap_without_truncating() {
        let value = "é".repeat(128);

        assert_eq!(
            Err(ListingSourceNameError::TooLong {
                max_bytes: ListingSourceName::MAX_BYTES,
                actual_bytes: 256,
            }),
            ListingSourceName::try_from(value.as_str())
        );
    }

    #[test]
    fn should_accept_listing_source_name_at_byte_cap_without_truncating() {
        let value = format!("{}a", "é".repeat(127));
        let name = ListingSourceName::try_from(value.as_str());

        assert_eq!(Ok(value), name.map(|value| value.to_string()));
    }
}
