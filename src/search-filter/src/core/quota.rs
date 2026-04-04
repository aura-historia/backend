use user::core::tier::UserTier;

pub trait SearchFilterQuota {
    fn search_filter_quota(&self) -> u32;
    fn search_filter_match_quota(&self) -> u32;
}

impl SearchFilterQuota for UserTier {
    fn search_filter_quota(&self) -> u32 {
        match self {
            UserTier::Free => 1,
            UserTier::Pro => 5,
            UserTier::Ultimate => u32::MAX,
        }
    }

    fn search_filter_match_quota(&self) -> u32 {
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
    use crate::core::quota::SearchFilterQuota;

    #[test]
    fn should_enforce_search_filter_quota() {
        assert_eq!(UserTier::Free.search_filter_quota(), 1);
        assert_eq!(UserTier::Pro.search_filter_quota(), 5);
        assert_eq!(UserTier::Ultimate.search_filter_quota(), u32::MAX);
    }

    #[test]
    fn should_enforce_search_filter_match_quota() {
        assert_eq!(UserTier::Free.search_filter_match_quota(), 10);
        assert_eq!(UserTier::Pro.search_filter_match_quota(), u32::MAX);
        assert_eq!(UserTier::Ultimate.search_filter_match_quota(), u32::MAX);
    }
}
