#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, strum_macros::EnumIter)]
pub enum ProductLifecycle {
    #[default]
    Active,
    Deleted,
}

impl ProductLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleted => "DELETED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_unique_canonical_product_lifecycle_identifiers() {
        let identifiers = ProductLifecycle::iter()
            .map(ProductLifecycle::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ProductLifecycle::iter().count(), identifiers.len());
        assert_eq!("ACTIVE", ProductLifecycle::Active.as_str());
        assert_eq!("DELETED", ProductLifecycle::Deleted.as_str());
    }
}
