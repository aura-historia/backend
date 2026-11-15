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
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|tier| tier.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "FREE",
            Self::Pro => "PRO",
            Self::Ultimate => "ULTIMATE",
        }
    }

    /// How many saved searches may be active at once. The create and reactivate checks and the
    /// tier reconciliation both read it from here, so the two cannot disagree.
    ///
    /// Every tier is bounded because each active saved search is classified every day: an
    /// unbounded tier would leave one account's matching cost and run time unbounded too.
    pub fn active_search_filter_quota(self) -> usize {
        match self {
            Self::Free => 1,
            Self::Pro => 5,
            Self::Ultimate => 20,
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
    fn should_apply_active_search_filter_quotas_by_tier() {
        assert_eq!(1, UserTier::Free.active_search_filter_quota());
        assert_eq!(5, UserTier::Pro.active_search_filter_quota());
        assert_eq!(20, UserTier::Ultimate.active_search_filter_quota());
    }

    #[test]
    fn should_have_unique_user_tier_identifiers() {
        let identifiers = UserTier::iter()
            .map(UserTier::as_str)
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(UserTier::iter().count(), identifiers.len());
    }

    #[test]
    fn should_round_trip_canonical_user_tier_identifiers() {
        for tier in UserTier::iter() {
            assert_eq!(Some(tier), UserTier::from_code(tier.as_str()));
        }
        assert_eq!(None, UserTier::from_code("pro"));
    }
}
