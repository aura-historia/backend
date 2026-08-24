#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, strum_macros::EnumIter)]
pub enum ShopLifecycle {
    #[default]
    Drafted,
    Published,
    Discarded,
}

impl ShopLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drafted => "DRAFTED",
            Self::Published => "PUBLISHED",
            Self::Discarded => "DISCARDED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_default_to_drafted() {
        assert_eq!(ShopLifecycle::Drafted, ShopLifecycle::default());
    }

    #[test]
    fn should_use_unique_canonical_lifecycle_identifiers() {
        let identifiers = ShopLifecycle::iter()
            .map(ShopLifecycle::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ShopLifecycle::iter().count(), identifiers.len());
        assert_eq!("DRAFTED", ShopLifecycle::Drafted.as_str());
        assert_eq!("PUBLISHED", ShopLifecycle::Published.as_str());
        assert_eq!("DISCARDED", ShopLifecycle::Discarded.as_str());
    }
}
