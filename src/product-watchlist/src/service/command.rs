use common::resource_state::domain::ResourceState;

#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateWatchlistProductCommand {
    pub notifications: Option<bool>,
    pub state: Option<ResourceState>,
}

impl UpdateWatchlistProductCommand {
    pub fn is_empty(&self) -> bool {
        self.notifications.is_none() && self.state.is_none()
    }
}
