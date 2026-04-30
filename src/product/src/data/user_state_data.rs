use crate::core::user_state::{
    NotificationUserState, ProductUserState, ProhibitedContentUserState, SearchFilterUserState,
    WatchlistUserState,
};
use common::event_id::EventId;
use common::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUserStateData {
    pub watchlist: WatchlistUserStateData,
    pub prohibited_content: ProhibitedContentUserStateData,
    pub notification: NotificationUserStateData,
    pub search_filter: SearchFilterUserStateData,
}
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistUserStateData {
    pub watching: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProhibitedContentUserStateData {
    pub consent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationUserStateData {
    pub seen: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_event_id: Option<EventId>,
}

impl Default for NotificationUserStateData {
    fn default() -> Self {
        Self {
            seen: true,
            origin_event_id: None,
        }
    }
}

impl From<ProductUserState> for ProductUserStateData {
    fn from(value: ProductUserState) -> Self {
        ProductUserStateData {
            watchlist: value.watchlist.into(),
            prohibited_content: value.prohibited_content.into(),
            notification: value.notification.into(),
            search_filter: value.search_filter.into(),
        }
    }
}

impl From<ProhibitedContentUserState> for ProhibitedContentUserStateData {
    fn from(value: ProhibitedContentUserState) -> Self {
        ProhibitedContentUserStateData {
            consent: value.consent,
        }
    }
}

impl From<WatchlistUserState> for WatchlistUserStateData {
    fn from(value: WatchlistUserState) -> Self {
        WatchlistUserStateData {
            watching: value.watching,
            notifications: value.notifications,
        }
    }
}

impl From<NotificationUserState> for NotificationUserStateData {
    fn from(value: NotificationUserState) -> Self {
        NotificationUserStateData {
            seen: value.seen,
            origin_event_id: value.origin_event_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterUserStateData {
    pub matched: bool,
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_search_filter_id: Option<UserSearchFilterId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_search_filter_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub match_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub match_feedback: Option<bool>,
}

impl From<SearchFilterUserState> for SearchFilterUserStateData {
    fn from(value: SearchFilterUserState) -> Self {
        SearchFilterUserStateData {
            matched: value.matched,
            hidden: value.hidden,
            user_search_filter_id: value.user_search_filter_id,
            user_search_filter_name: value.user_search_filter_name.map(Into::into),
            match_reason: value.match_reason.map(Into::into),
            match_feedback: value.match_feedback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::enhanced_match_reason::EnhancedMatchReason;

    #[test]
    fn should_serialize_product_user_state_data_with_notification() {
        let data = ProductUserStateData {
            watchlist: WatchlistUserStateData {
                watching: true,
                notifications: false,
            },
            prohibited_content: ProhibitedContentUserStateData { consent: true },
            notification: NotificationUserStateData {
                seen: false,
                origin_event_id: None,
            },
            search_filter: SearchFilterUserStateData::default(),
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["notification"]["seen"], false);
        assert_eq!(json["watchlist"]["watching"], true);
        assert_eq!(json["prohibitedContent"]["consent"], true);
        assert!(json["notification"].get("originEventId").is_none());
        assert_eq!(json["searchFilter"]["matched"], false);
        assert_eq!(json["searchFilter"]["hidden"], false);
    }

    #[test]
    fn should_default_notification_user_state_data_seen_to_true() {
        let data = NotificationUserStateData::default();
        assert!(data.seen);
        assert!(data.origin_event_id.is_none());
    }

    #[test]
    fn should_convert_notification_user_state_to_data() {
        let state = NotificationUserState {
            seen: false,
            origin_event_id: None,
        };
        let data: NotificationUserStateData = state.into();
        assert!(!data.seen);
        assert!(data.origin_event_id.is_none());
    }

    #[test]
    fn should_convert_notification_user_state_with_event_id_to_data() {
        let event_id = EventId::new();
        let state = NotificationUserState {
            seen: false,
            origin_event_id: Some(event_id),
        };
        let data: NotificationUserStateData = state.into();
        assert!(!data.seen);
        assert_eq!(data.origin_event_id, Some(event_id));
    }

    #[test]
    fn should_serialize_origin_event_id_when_present() {
        let event_id = EventId::new();
        let data = NotificationUserStateData {
            seen: false,
            origin_event_id: Some(event_id),
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["seen"], false);
        assert_eq!(
            json["originEventId"].as_str().unwrap(),
            event_id.to_string()
        );
    }

    #[test]
    fn should_omit_origin_event_id_when_none() {
        let data = NotificationUserStateData {
            seen: true,
            origin_event_id: None,
        };
        let json = serde_json::to_value(data).unwrap();
        assert!(json.get("originEventId").is_none());
    }

    #[test]
    fn should_convert_product_user_state_to_data_including_notification() {
        let state = ProductUserState {
            watchlist: WatchlistUserState {
                watching: true,
                notifications: true,
            },
            prohibited_content: ProhibitedContentUserState { consent: false },
            notification: NotificationUserState {
                seen: false,
                origin_event_id: None,
            },
            search_filter: SearchFilterUserState::default(),
        };
        let data: ProductUserStateData = state.into();
        assert!(data.watchlist.watching);
        assert!(data.watchlist.notifications);
        assert!(!data.prohibited_content.consent);
        assert!(!data.notification.seen);
        assert!(data.notification.origin_event_id.is_none());
        assert!(!data.search_filter.matched);
    }

    #[test]
    fn should_default_search_filter_user_state_data_to_not_matched() {
        let data = SearchFilterUserStateData::default();
        assert!(!data.matched);
        assert!(!data.hidden);
        assert!(data.user_search_filter_id.is_none());
        assert!(data.user_search_filter_name.is_none());
        assert!(data.match_reason.is_none());
    }

    #[test]
    fn should_convert_search_filter_user_state_to_data() {
        use common::user_search_filter_name::UserSearchFilterName;
        let filter_id = UserSearchFilterId::new();
        let reason = EnhancedMatchReason::from("matched because of vintage style");
        let state = SearchFilterUserState {
            matched: true,
            hidden: false,
            user_search_filter_id: Some(filter_id),
            user_search_filter_name: Some(UserSearchFilterName::from("Antique Watches")),
            match_reason: Some(reason),
            match_feedback: Some(true),
        };
        let data: SearchFilterUserStateData = state.into();
        assert!(data.matched);
        assert!(!data.hidden);
        assert_eq!(data.user_search_filter_id, Some(filter_id));
        assert_eq!(
            data.user_search_filter_name.as_deref(),
            Some("Antique Watches")
        );
        assert_eq!(
            data.match_reason.as_deref(),
            Some("matched because of vintage style")
        );
    }

    #[test]
    fn should_convert_search_filter_user_state_to_data_when_hidden() {
        let state = SearchFilterUserState {
            matched: true,
            hidden: true,
            user_search_filter_id: None,
            user_search_filter_name: None,
            match_reason: None,
            match_feedback: Some(false),
        };
        let data: SearchFilterUserStateData = state.into();
        assert!(data.matched);
        assert!(data.hidden);
    }

    #[test]
    fn should_serialize_search_filter_user_state_data_when_matched() {
        let filter_id = UserSearchFilterId::new();
        let data = SearchFilterUserStateData {
            matched: true,
            hidden: false,
            user_search_filter_id: Some(filter_id),
            user_search_filter_name: Some("My Filter".to_string()),
            match_reason: Some("vintage match".to_string()),
            match_feedback: Some(true),
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["matched"], true);
        assert_eq!(json["hidden"], false);
        assert_eq!(
            json["userSearchFilterId"].as_str().unwrap(),
            filter_id.to_string()
        );
        assert_eq!(json["userSearchFilterName"], "My Filter");
        assert_eq!(json["matchReason"], "vintage match");
    }

    #[test]
    fn should_omit_optional_fields_when_search_filter_not_matched() {
        let data = SearchFilterUserStateData {
            matched: false,
            hidden: false,
            user_search_filter_id: None,
            user_search_filter_name: None,
            match_reason: None,
            match_feedback: Some(true),
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["matched"], false);
        assert_eq!(json["hidden"], false);
        assert!(json.get("userSearchFilterId").is_none());
        assert!(json.get("userSearchFilterName").is_none());
        assert!(json.get("matchReason").is_none());
    }

    #[test]
    fn should_serialize_search_filter_hidden_field() {
        let data = SearchFilterUserStateData {
            matched: true,
            hidden: true,
            user_search_filter_id: None,
            user_search_filter_name: None,
            match_reason: None,
            match_feedback: Some(true),
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["matched"], true);
        assert_eq!(json["hidden"], true);
    }
}
