#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, strum_macros::EnumIter,
)]
pub enum UserTier {
    #[default]
    Free,
    Pro,
    Ultimate,
}

impl UserTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "FREE",
            Self::Pro => "PRO",
            Self::Ultimate => "ULTIMATE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserTier;
    use strum::IntoEnumIterator;

    #[test]
    fn should_default_user_tier_to_free() {
        assert_eq!(UserTier::Free, UserTier::default());
    }

    #[test]
    fn should_order_user_tiers() {
        assert!(UserTier::Free < UserTier::Pro);
        assert!(UserTier::Pro < UserTier::Ultimate);
    }

    #[test]
    fn should_sort_user_tiers() {
        let mut tiers = vec![UserTier::Ultimate, UserTier::Free, UserTier::Pro];
        tiers.sort();
        assert_eq!(
            tiers,
            vec![UserTier::Free, UserTier::Pro, UserTier::Ultimate]
        );
    }

    #[test]
    fn should_render_canonical_user_tier_identifiers() {
        assert_eq!("FREE", UserTier::Free.as_str());
        assert_eq!("PRO", UserTier::Pro.as_str());
        assert_eq!("ULTIMATE", UserTier::Ultimate.as_str());
    }

    #[test]
    fn should_have_unique_user_tier_identifiers() {
        let identifiers = UserTier::iter()
            .map(UserTier::as_str)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(UserTier::iter().count(), identifiers.len());
    }
}
