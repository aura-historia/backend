use crate::error::{ApiError, BAD_BODY_VALUE};
use axum::response::{IntoResponse, Response};
use common::notification_id::NotificationId;
use notification_core::notification::{
    LocalizedNotificationContent, LocalizedNotificationWatchlistChange, PartnerApplicationDecision,
};
use notification_service::use_cases::queries::list_notifications::ListedNotification;
use serde::{Deserialize, Serialize};

use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNotificationSeenData {
    pub seen: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNotificationsSeenData {
    pub notification_ids: Vec<Uuid>,
    pub seen: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationData {
    notification_id: Uuid,
    seen: bool,
    created: String,
    updated: String,
    kind: NotificationKindData,
    payload: NotificationContentData,
}

impl From<ListedNotification> for NotificationData {
    fn from(value: ListedNotification) -> Self {
        Self {
            notification_id: Uuid::from(value.notification_id),
            seen: value.seen,
            created: value.created.to_string(),
            updated: value.updated.to_string(),
            kind: NotificationKindData::from(&value.content),
            payload: value.content.into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
enum NotificationContentData {
    Watchlist {
        product_id: Uuid,
    },
    SearchFilter {
        product_id: Uuid,
        user_search_filter_id: Uuid,
        user_search_filter_name: String,
    },
    PartnerApplication {
        partner_shop_application_id: Uuid,
        decision: PartnerApplicationDecisionData,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum NotificationKindData {
    WatchlistPriceChanged,
    WatchlistStateChanged,
    SearchFilterMatch,
    PartnerApplicationApproved,
    PartnerApplicationRejected,
}

impl From<&LocalizedNotificationContent> for NotificationKindData {
    fn from(value: &LocalizedNotificationContent) -> Self {
        match value {
            LocalizedNotificationContent::Watchlist {
                change: LocalizedNotificationWatchlistChange::PriceChange { .. },
                ..
            } => Self::WatchlistPriceChanged,
            LocalizedNotificationContent::Watchlist { .. } => Self::WatchlistStateChanged,
            LocalizedNotificationContent::SearchFilter { .. } => Self::SearchFilterMatch,
            LocalizedNotificationContent::PartnerApplication {
                decision: PartnerApplicationDecision::Approved,
                ..
            } => Self::PartnerApplicationApproved,
            LocalizedNotificationContent::PartnerApplication { .. } => {
                Self::PartnerApplicationRejected
            }
        }
    }
}

impl From<LocalizedNotificationContent> for NotificationContentData {
    fn from(value: LocalizedNotificationContent) -> Self {
        match value {
            LocalizedNotificationContent::Watchlist { product_id, .. } => Self::Watchlist {
                product_id: Uuid::from(product_id),
            },
            LocalizedNotificationContent::SearchFilter {
                product_id,
                user_search_filter_id,
                user_search_filter_name,
                ..
            } => Self::SearchFilter {
                product_id: Uuid::from(product_id),
                user_search_filter_id: Uuid::from(user_search_filter_id),
                user_search_filter_name: user_search_filter_name.to_string(),
            },
            LocalizedNotificationContent::PartnerApplication {
                partner_shop_application_id,
                decision,
                ..
            } => Self::PartnerApplication {
                partner_shop_application_id: Uuid::from(partner_shop_application_id),
                decision: decision.into(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PartnerApplicationDecisionData {
    Approved,
    Rejected,
}

impl From<PartnerApplicationDecision> for PartnerApplicationDecisionData {
    fn from(value: PartnerApplicationDecision) -> Self {
        match value {
            PartnerApplicationDecision::Approved => Self::Approved,
            PartnerApplicationDecision::Rejected => Self::Rejected,
        }
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Request body is required.")
            .into_response());
    }
    serde_json::from_str(body).map_err(|error| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(error.to_string())
            .into_response()
    })
}

pub(crate) fn notification_id(value: Uuid) -> NotificationId {
    NotificationId::from(value)
}
