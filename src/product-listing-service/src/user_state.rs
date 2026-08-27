use notification_core::notification_id::NotificationId;
use search_filter_core::{
    enhanced_match_reason::EnhancedMatchReason, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingUserState {
    pub watchlist: WatchlistUserState,
    pub content_visibility: ContentVisibilityUserState,
    pub notification: NotificationUserState,
    pub search_filter: SearchFilterUserState,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WatchlistUserState {
    pub watching: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ContentVisibilityUserState {
    pub show_unassessed_or_sensitive_content: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NotificationUserState {
    pub unseen_notification_ids: Vec<NotificationId>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchFilterUserState {
    pub matched: bool,
    pub hidden: bool,
    pub user_search_filter_id: Option<UserSearchFilterId>,
    pub user_search_filter_name: Option<UserSearchFilterName>,
    pub match_reason: Option<EnhancedMatchReason>,
    pub match_feedback: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_notification_user_state_seen_to_true() {
        let state = NotificationUserState::default();
        assert!(state.unseen_notification_ids.is_empty());
    }

    #[test]
    fn should_default_product_user_state_notification_seen_to_true() {
        let state = ProductListingUserState::default();
        assert!(state.notification.unseen_notification_ids.is_empty());
    }

    #[test]
    fn should_default_search_filter_user_state_to_not_matched() {
        let state = SearchFilterUserState::default();
        assert!(!state.matched);
        assert!(!state.hidden);
        assert!(state.user_search_filter_id.is_none());
        assert!(state.user_search_filter_name.is_none());
        assert!(state.match_reason.is_none());
    }

    #[test]
    fn should_default_product_user_state_search_filter_to_not_matched() {
        let state = ProductListingUserState::default();
        assert!(!state.search_filter.matched);
        assert!(!state.search_filter.hidden);
        assert!(state.search_filter.user_search_filter_id.is_none());
        assert!(state.search_filter.user_search_filter_name.is_none());
        assert!(state.search_filter.match_reason.is_none());
    }
}
