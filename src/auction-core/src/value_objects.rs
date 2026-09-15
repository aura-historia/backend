use std::fmt;

const MAX_SOURCE_AUCTION_ID_BYTES: usize = 512;
const MAX_AUCTION_NAME_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceAuctionId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSourceAuctionId {
    #[error("source auction ID cannot be blank")]
    Blank,
    #[error("source auction ID cannot contain a NUL character")]
    ContainsNul,
    #[error("source auction ID exceeds {MAX_SOURCE_AUCTION_ID_BYTES} UTF-8 bytes")]
    TooLong,
}

impl SourceAuctionId {
    fn parse(value: &str) -> Result<Self, InvalidSourceAuctionId> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(InvalidSourceAuctionId::Blank);
        }
        if trimmed.contains('\0') {
            return Err(InvalidSourceAuctionId::ContainsNul);
        }
        if trimmed.len() > MAX_SOURCE_AUCTION_ID_BYTES {
            return Err(InvalidSourceAuctionId::TooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl TryFrom<&str> for SourceAuctionId {
    type Error = InvalidSourceAuctionId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for SourceAuctionId {
    type Error = InvalidSourceAuctionId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl AsRef<str> for SourceAuctionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceAuctionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl From<SourceAuctionId> for String {
    fn from(value: SourceAuctionId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAuctionName {
    #[error("auction name cannot be blank")]
    Blank,
    #[error("auction name cannot contain a NUL character")]
    ContainsNul,
    #[error("auction name exceeds {MAX_AUCTION_NAME_BYTES} UTF-8 bytes")]
    TooLong,
}

impl AuctionName {
    fn parse(value: &str) -> Result<Self, InvalidAuctionName> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(InvalidAuctionName::Blank);
        }
        if trimmed.contains('\0') {
            return Err(InvalidAuctionName::ContainsNul);
        }
        if trimmed.len() > MAX_AUCTION_NAME_BYTES {
            return Err(InvalidAuctionName::TooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl TryFrom<&str> for AuctionName {
    type Error = InvalidAuctionName;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for AuctionName {
    type Error = InvalidAuctionName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl AsRef<str> for AuctionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AuctionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_ref())
    }
}

impl From<AuctionName> for String {
    fn from(value: AuctionName) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::{AuctionName, InvalidAuctionName, InvalidSourceAuctionId, SourceAuctionId};
    use rstest::rstest;

    #[test]
    fn should_trim_source_auction_id_without_changing_its_namespace_text() {
        let value = SourceAuctionId::try_from("\u{2003} Auc  #42/A \u{2002}")
            .unwrap_or_else(|error| panic!("valid source auction ID: {error}"));

        assert_eq!("Auc  #42/A", value.as_ref());
    }

    #[rstest]
    #[case("", InvalidSourceAuctionId::Blank)]
    #[case(" \t\n\u{2003} ", InvalidSourceAuctionId::Blank)]
    #[case("id\0value", InvalidSourceAuctionId::ContainsNul)]
    fn should_reject_invalid_source_auction_ids(
        #[case] value: &str,
        #[case] expected: InvalidSourceAuctionId,
    ) {
        assert_eq!(Err(expected), SourceAuctionId::try_from(value));
    }

    #[test]
    fn should_enforce_source_auction_id_utf8_byte_limit() {
        assert!(SourceAuctionId::try_from("é".repeat(256)).is_ok());
        assert_eq!(
            Err(InvalidSourceAuctionId::TooLong),
            SourceAuctionId::try_from("é".repeat(257))
        );
    }

    #[test]
    fn should_validate_auction_name_without_truncating() {
        assert_eq!(
            "Autumn Decorative Arts",
            AuctionName::try_from("  Autumn Decorative Arts  ")
                .unwrap_or_else(|error| panic!("valid auction name: {error}"))
                .as_ref()
        );
        assert_eq!(
            Err(InvalidAuctionName::Blank),
            AuctionName::try_from("\u{2003}")
        );
        assert_eq!(
            Err(InvalidAuctionName::ContainsNul),
            AuctionName::try_from("name\0")
        );
        assert_eq!(
            Err(InvalidAuctionName::TooLong),
            AuctionName::try_from("é".repeat(257))
        );
    }
}
