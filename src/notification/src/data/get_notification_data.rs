use crate::core::notification::{
    LocalizedNotification, LocalizedNotificationPayload, LocalizedNotificationWatchlistPayload,
};
use crate::core::notification_id::NotificationId;
use common::{
    event_id::EventId, price::data::PriceData, product_id::ProductId,
    product_state::domain::ProductState, shop_id::ShopId, shop_name::ShopName,
    shops_product_id::ShopsProductId, slug_id::SlugId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNotificationData {
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub payload: NotificationPayloadData,
    pub seen: bool,
    pub external: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationPayloadData {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: String,
        watchlist_payload: WatchlistPayloadData,
    },
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatchlistPayloadData {
    PriceChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_price: Option<PriceData>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_price: Option<PriceData>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}

impl From<LocalizedNotificationWatchlistPayload> for WatchlistPayloadData {
    fn from(payload: LocalizedNotificationWatchlistPayload) -> Self {
        match payload {
            LocalizedNotificationWatchlistPayload::PriceChange {
                old_price,
                new_price,
            } => WatchlistPayloadData::PriceChange {
                old_price: old_price.map(PriceData::from),
                new_price: new_price.map(PriceData::from),
            },
            LocalizedNotificationWatchlistPayload::StateChange {
                old_state,
                new_state,
            } => WatchlistPayloadData::StateChange {
                old_state,
                new_state,
            },
        }
    }
}

impl From<LocalizedNotificationPayload> for NotificationPayloadData {
    fn from(payload: LocalizedNotificationPayload) -> Self {
        match payload {
            LocalizedNotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                watchlist_payload,
            } => NotificationPayloadData::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title: title.payload.to_string(),
                watchlist_payload: watchlist_payload.into(),
            },
        }
    }
}

impl From<LocalizedNotification> for GetNotificationData {
    fn from(notification: LocalizedNotification) -> Self {
        GetNotificationData {
            origin_event_id: notification.origin_event_id,
            notification_id: notification.notification_id,
            payload: notification.notification_payload.into(),
            seen: notification.seen,
            external: notification.external,
            created: notification.created,
            updated: notification.updated,
        }
    }
}
