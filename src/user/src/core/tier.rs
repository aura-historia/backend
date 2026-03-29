#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UserTier {
    #[default]
    Free,
}

impl UserTier {
    pub fn watchlist_limit(&self) -> usize {
        match self {
            UserTier::Free => 5,
        }
    }

    pub fn search_filter_limit(&self) -> usize {
        match self {
            UserTier::Free => 5,
        }
    }
}
