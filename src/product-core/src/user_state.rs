use common::enhanced_match_reason::EnhancedMatchReason;
use common::notification_id::NotificationId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductUserState {
    pub watchlist: WatchlistUserState,
    pub prohibited_content: ProhibitedContentUserState,
    pub notification: NotificationUserState,
    pub search_filter: SearchFilterUserState,
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
        let state = ProductUserState::default();
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
        let state = ProductUserState::default();
        assert!(!state.search_filter.matched);
        assert!(!state.search_filter.hidden);
        assert!(state.search_filter.user_search_filter_id.is_none());
        assert!(state.search_filter.user_search_filter_name.is_none());
        assert!(state.search_filter.match_reason.is_none());
    }
}
