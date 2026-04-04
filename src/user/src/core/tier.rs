#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum UserTier {
    #[default]
    Free,
    Pro,
    Ultimate,
}

impl UserTier {
    pub fn watchlist_limit(&self) -> u32 {
        match self {
            UserTier::Free => 20,
            UserTier::Pro => 100,
            UserTier::Ultimate => u32::MAX,
        }
    }

    pub fn search_filter_limit(&self) -> u32 {
        match self {
            UserTier::Free => 1,
            UserTier::Pro => 5,
            UserTier::Ultimate => u32::MAX,
        }
    }

    pub fn search_filter_matches_limit(&self) -> u32 {
        match self {
            UserTier::Free => 10,
            UserTier::Pro => u32::MAX,
            UserTier::Ultimate => u32::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserTier;

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
    fn should_enforce_watchlist_limit() {
        assert_eq!(UserTier::Free.watchlist_limit(), 20);
        assert_eq!(UserTier::Pro.watchlist_limit(), 100);
        assert_eq!(UserTier::Ultimate.watchlist_limit(), u32::MAX);
    }

    #[test]
    fn should_enforce_search_filter_limit() {
        assert_eq!(UserTier::Free.search_filter_limit(), 1);
        assert_eq!(UserTier::Pro.search_filter_limit(), 5);
        assert_eq!(UserTier::Ultimate.search_filter_limit(), u32::MAX);
    }

    #[test]
    fn should_enforce_search_filter_matches_limit() {
        assert_eq!(UserTier::Free.search_filter_matches_limit(), 10);
        assert_eq!(UserTier::Pro.search_filter_matches_limit(), u32::MAX);
        assert_eq!(UserTier::Ultimate.search_filter_matches_limit(), u32::MAX);
    }
}
