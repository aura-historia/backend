#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateWatchlistProductCommand {
    pub notifications: Option<bool>,
}

impl UpdateWatchlistProductCommand {
    pub fn is_empty(&self) -> bool {
        self.notifications.is_none()
    }
}
