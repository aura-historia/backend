use common::event_id::EventId;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProductUserState {
    pub watchlist: WatchlistUserState,
    pub prohibited_content: ProhibitedContentUserState,
    pub notification: NotificationUserState,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotificationUserState {
    pub seen: bool,
    pub origin_event_id: Option<EventId>,
}

impl Default for NotificationUserState {
    fn default() -> Self {
        Self {
            seen: true,
            origin_event_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_notification_user_state_seen_to_true() {
        let state = NotificationUserState::default();
        assert!(state.seen);
        assert!(state.origin_event_id.is_none());
    }

    #[test]
    fn should_default_product_user_state_notification_seen_to_true() {
        let state = ProductUserState::default();
        assert!(state.notification.seen);
        assert!(state.notification.origin_event_id.is_none());
    }
}
