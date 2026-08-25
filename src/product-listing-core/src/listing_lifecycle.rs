#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ListingLifecycle {
    Active,
    Withdrawn,
}

impl ListingLifecycle {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|lifecycle| lifecycle.as_str() == value)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Withdrawn => "WITHDRAWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_unique_canonical_codes() {
        let codes = ListingLifecycle::iter()
            .map(ListingLifecycle::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ListingLifecycle::iter().count(), codes.len());
        for lifecycle in ListingLifecycle::iter() {
            assert_eq!(
                Some(lifecycle),
                ListingLifecycle::from_code(lifecycle.as_str())
            );
        }
        assert_eq!(None, ListingLifecycle::from_code("active"));
    }
}
