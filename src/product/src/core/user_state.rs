#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductUserState {
    pub watchlist: WatchlistUserState,
    pub prohibited_content: ProhibitedContentUserState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WatchlistUserState {
    pub watching: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProhibitedContentUserState {
    pub consent: bool,
}
