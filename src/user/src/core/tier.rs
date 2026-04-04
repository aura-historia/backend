#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
