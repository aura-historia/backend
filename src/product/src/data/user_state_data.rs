use crate::core::user_state::{
    NotificationUserState, ProductUserState, ProhibitedContentUserState, WatchlistUserState,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductUserStateData {
    pub watchlist: WatchlistUserStateData,
    pub prohibited_content: ProhibitedContentUserStateData,
    pub notification: NotificationUserStateData,
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
}

impl Default for NotificationUserStateData {
    fn default() -> Self {
        Self { seen: true }
    }
}

impl From<ProductUserState> for ProductUserStateData {
    fn from(value: ProductUserState) -> Self {
        ProductUserStateData {
            watchlist: value.watchlist.into(),
            prohibited_content: value.prohibited_content.into(),
            notification: value.notification.into(),
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
        NotificationUserStateData { seen: value.seen }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_serialize_product_user_state_data_with_notification() {
        let data = ProductUserStateData {
            watchlist: WatchlistUserStateData {
                watching: true,
                notifications: false,
            },
            prohibited_content: ProhibitedContentUserStateData { consent: true },
            notification: NotificationUserStateData { seen: false },
        };
        let json = serde_json::to_value(data).unwrap();
        assert_eq!(json["notification"]["seen"], false);
        assert_eq!(json["watchlist"]["watching"], true);
        assert_eq!(json["prohibitedContent"]["consent"], true);
    }

    #[test]
    fn should_default_notification_user_state_data_seen_to_true() {
        let data = NotificationUserStateData::default();
        assert!(data.seen);
    }

    #[test]
    fn should_convert_notification_user_state_to_data() {
        let state = NotificationUserState { seen: false };
        let data: NotificationUserStateData = state.into();
        assert!(!data.seen);
    }

    #[test]
    fn should_convert_product_user_state_to_data_including_notification() {
        let state = ProductUserState {
            watchlist: WatchlistUserState {
                watching: true,
                notifications: true,
            },
            prohibited_content: ProhibitedContentUserState { consent: false },
            notification: NotificationUserState { seen: false },
        };
        let data: ProductUserStateData = state.into();
        assert!(data.watchlist.watching);
        assert!(data.watchlist.notifications);
        assert!(!data.prohibited_content.consent);
        assert!(!data.notification.seen);
    }
}
