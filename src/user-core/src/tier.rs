#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum UserTier {
    #[default]
    Free,
    Pro,
    Ultimate,
}

#[cfg(test)]
mod tests {
    use super::UserTier;

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
}
