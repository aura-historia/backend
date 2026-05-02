use crate::core::watchlist_product_state::WatchlistProductState;

#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateWatchlistProductCommand {
    pub notifications: Option<bool>,
    pub state: Option<WatchlistProductState>,
}

impl UpdateWatchlistProductCommand {
    pub fn is_empty(&self) -> bool {
        self.notifications.is_none() && self.state.is_none()
    }
}
