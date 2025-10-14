#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateWatchlistItemCommand {
    pub notifications: Option<bool>,
}

impl UpdateWatchlistItemCommand {
    pub fn is_empty(&self) -> bool {
        self.notifications.is_none()
    }
}
