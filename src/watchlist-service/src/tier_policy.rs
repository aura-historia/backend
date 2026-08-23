use user_core::tier::UserTier;

pub(crate) fn active_watchlist_quota(tier: UserTier) -> Option<usize> {
    match tier {
        UserTier::Free => Some(20),
        UserTier::Pro => Some(100),
        UserTier::Ultimate => None,
    }
}

#[cfg(test)]
mod tests {
    use super::active_watchlist_quota;
    use user_core::tier::UserTier;

    #[test]
    fn should_match_legacy_active_watchlist_quotas() {
        assert_eq!(Some(20), active_watchlist_quota(UserTier::Free));
        assert_eq!(Some(100), active_watchlist_quota(UserTier::Pro));
        assert_eq!(None, active_watchlist_quota(UserTier::Ultimate));
    }
}
