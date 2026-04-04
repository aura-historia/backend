use user::core::tier::UserTier;

pub trait WatchlistQuota {
    fn watchlist_quota(&self) -> u32;
}

impl WatchlistQuota for UserTier {
    fn watchlist_quota(&self) -> u32 {
        match self {
            UserTier::Free => 20,
            UserTier::Pro => 100,
            UserTier::Ultimate => u32::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UserTier;
    use crate::core::quota::WatchlistQuota;

    #[test]
    fn should_enforce_watchlist_quota() {
        assert_eq!(UserTier::Free.watchlist_quota(), 20);
        assert_eq!(UserTier::Pro.watchlist_quota(), 100);
        assert_eq!(UserTier::Ultimate.watchlist_quota(), u32::MAX);
    }
}
