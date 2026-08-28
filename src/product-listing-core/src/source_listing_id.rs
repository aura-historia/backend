use std::fmt;

const MAX_SOURCE_LISTING_ID_BYTES: usize = 512;

#[derive(
    Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct SourceListingId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSourceListingId {
    #[error("source listing ID cannot be blank")]
    Blank,

    #[error("source listing ID exceeds {MAX_SOURCE_LISTING_ID_BYTES} UTF-8 bytes")]
    TooLong,
}

impl SourceListingId {
    fn parse(value: &str) -> Result<Self, InvalidSourceListingId> {
        let value = value.trim();
        if value.is_empty() {
            return Err(InvalidSourceListingId::Blank);
        }
        if value.len() > MAX_SOURCE_LISTING_ID_BYTES {
            return Err(InvalidSourceListingId::TooLong);
        }

        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for SourceListingId {
    type Error = InvalidSourceListingId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl TryFrom<&str> for SourceListingId {
    type Error = InvalidSourceListingId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl AsRef<str> for SourceListingId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceListingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl From<SourceListingId> for String {
    fn from(value: SourceListingId) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{InvalidSourceListingId, SourceListingId};
    use rstest::rstest;

    #[test]
    fn should_trim_outer_unicode_whitespace_and_preserve_the_rest() {
        let source_listing_id = SourceListingId::try_from("\u{2003} SKU  #42/Blue \u{2002}")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        assert_eq!(source_listing_id.as_ref(), "SKU  #42/Blue");
        assert_eq!(source_listing_id.to_string(), "SKU  #42/Blue");
        assert_eq!(String::from(source_listing_id), "SKU  #42/Blue");
    }

    #[rstest]
    #[case("")]
    #[case(" \t\n\u{2003} ")]
    fn should_reject_blank_source_listing_id(#[case] value: &str) {
        assert_eq!(
            SourceListingId::try_from(value),
            Err(InvalidSourceListingId::Blank)
        );
    }

    #[test]
    fn should_accept_source_listing_id_at_utf8_byte_limit() {
        let value = "é".repeat(256);

        let source_listing_id = SourceListingId::try_from(value);

        assert!(source_listing_id.is_ok());
    }

    #[test]
    fn should_reject_source_listing_id_over_utf8_byte_limit() {
        let value = "é".repeat(257);

        assert_eq!(
            SourceListingId::try_from(value),
            Err(InvalidSourceListingId::TooLong)
        );
    }
}
