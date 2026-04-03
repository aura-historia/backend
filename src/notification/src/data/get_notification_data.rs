use crate::core::notification::{
    LocalizedNotification, LocalizedNotificationPayload, LocalizedNotificationWatchlistPayload,
};
use crate::core::notification_id::NotificationId;
use common::{
    event_id::EventId, language::data::LocalizedTextData, price::data::PriceData,
    product_id::ProductId, shop_id::ShopId, shop_name::ShopName, shops_product_id::ShopsProductId,
    slug_id::SlugId,
};
use product::data::product_image_data::ProductImageData;
use product::data::product_state_data::ProductStateData;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNotificationData {
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub payload: NotificationPayloadData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ProductImageData>,
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
        title: LocalizedTextData,
        watchlist_payload: WatchlistPayloadData,
    },
    #[serde(rename_all = "camelCase")]
    SearchFilter {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: LocalizedTextData,
        search_filter_payload: SearchFilterPayloadData,
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
        old_state: ProductStateData,
        new_state: ProductStateData,
    },
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterPayloadData {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_search_filter_name: UserSearchFilterName,
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
                old_state: old_state.into(),
                new_state: new_state.into(),
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
                title: title.into(),
                watchlist_payload: watchlist_payload.into(),
            },
            LocalizedNotificationPayload::SearchFilter {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                search_filter_payload,
            } => NotificationPayloadData::SearchFilter {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title: title.into(),
                search_filter_payload: SearchFilterPayloadData {
                    user_search_filter_id: search_filter_payload.user_search_filter_id,
                    user_search_filter_name: search_filter_payload.user_search_filter_name,
                },
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
            image: notification
                .image
                .map(|i| ProductImageData::from_with_consent(i, true)),
            seen: notification.seen,
            external: notification.external,
            created: notification.created,
            updated: notification.updated,
        }
    }
}
