#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum_macros::EnumCount,
    strum_macros::EnumIter,
)]
pub enum ProductState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl ProductState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listed => "LISTED",
            Self::Available => "AVAILABLE",
            Self::Reserved => "RESERVED",
            Self::Sold => "SOLD",
            Self::Removed => "REMOVED",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_unique_canonical_product_state_identifiers() {
        let identifiers = ProductState::iter()
            .map(ProductState::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ProductState::iter().count(), identifiers.len());
        assert_eq!("LISTED", ProductState::Listed.as_str());
        assert_eq!("AVAILABLE", ProductState::Available.as_str());
        assert_eq!("RESERVED", ProductState::Reserved.as_str());
        assert_eq!("SOLD", ProductState::Sold.as_str());
        assert_eq!("REMOVED", ProductState::Removed.as_str());
        assert_eq!("UNKNOWN", ProductState::Unknown.as_str());
    }
}
