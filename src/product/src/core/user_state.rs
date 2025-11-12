#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemUserState {
    pub watchlist: WatchlistUserState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WatchlistUserState {
    pub watching: bool,
    pub notifications: bool,
}
