use crate::core::notification::{
    LocalizedNotification, LocalizedNotificationPayload, LocalizedNotificationWatchlistPayload,
    NotificationPartnerApplicationPayload,
};
use crate::core::notification_id::NotificationId;
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::user_search_filter_id::UserSearchFilterId;
use common::{
    event_id::EventId, language::data::LocalizedTextData, price::data::PriceData,
    product_id::ProductId, shop_id::ShopId, shop_name::ShopName, shops_product_id::ShopsProductId,
    slug_id::SlugId,
};
use product::data::product_image_data::ProductImageData;
use product::data::product_state_data::ProductStateData;
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
    pub seen: bool,
    pub external: bool,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum NotificationPayloadData {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: LocalizedTextData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<ProductImageData>,
        url: url::Url,
        view_url: url::Url,
        watchlist_payload: WatchlistPayloadData,
    },
    SearchFilter {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: LocalizedTextData,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<ProductImageData>,
        url: url::Url,
        view_url: url::Url,
        search_filter_payload: SearchFilterPayloadData,
    },
    PartnerApplication {
        shop_name: ShopName,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<url::Url>,
        partner_application_payload: PartnerApplicationPayloadData,
    },
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
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

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum PartnerApplicationPayloadData {
    Approved {
        partner_application_id: PartnerShopApplicationId,
    },
    Rejected {
        partner_application_id: PartnerShopApplicationId,
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
                image,
                url,
                view_url,
                watchlist_payload,
            } => NotificationPayloadData::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title: title.into(),
                image: image.map(|i| ProductImageData::from_with_consent(i, true)),
                url,
                view_url,
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
                image,
                url,
                view_url,
                search_filter_payload,
            } => NotificationPayloadData::SearchFilter {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title: title.into(),
                image: image.map(|i| ProductImageData::from_with_consent(i, true)),
                url,
                view_url,
                search_filter_payload: SearchFilterPayloadData {
                    user_search_filter_id: search_filter_payload.user_search_filter_id,
                    user_search_filter_name: search_filter_payload.user_search_filter_name,
                },
            },
            LocalizedNotificationPayload::PartnerApplication {
                shop_name,
                image,
                partner_application_payload,
            } => NotificationPayloadData::PartnerApplication {
                shop_name,
                image,
                partner_application_payload: match partner_application_payload {
                    NotificationPartnerApplicationPayload::Approved {
                        partner_application_id,
                    } => PartnerApplicationPayloadData::Approved {
                        partner_application_id,
                    },
                    NotificationPartnerApplicationPayload::Rejected {
                        partner_application_id,
                    } => PartnerApplicationPayloadData::Rejected {
                        partner_application_id,
                    },
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
            seen: notification.seen,
            external: notification.external,
            created: notification.created,
            updated: notification.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GetNotificationData, NotificationPayloadData, PartnerApplicationPayloadData,
        SearchFilterPayloadData, WatchlistPayloadData,
    };
    use serde_json::json;

    #[test]
    fn should_roundtrip_watchlist_notification_payload_when_using_camel_case_fields() {
        let json = json!({
            "type": "WATCHLIST",
            "productId": "f6f6c303-f70f-4f6e-bdc5-20b0d22f41a1",
            "shopId": "1608beec-e0bc-4f3d-9794-24718c3f8558",
            "shopsProductId": "shop-product-123",
            "shopSlugId": "test-shop",
            "productSlugId": "test-product-abcdef",
            "shopName": "Test Shop",
            "title": {
                "text": "Test title",
                "language": "en"
            },
            "image": {
                "url": "https://test.example/product.jpg",
                "prohibitedContent": "NONE"
            },
            "url": "https://test.example/product",
            "viewUrl": "https://test.example/product?utm_source=aura_historia&utm_medium=referral",
            "watchlistPayload": {
                "type": "PRICE_CHANGE",
                "oldPrice": {
                    "currency": "EUR",
                    "amount": 1000
                },
                "newPrice": {
                    "currency": "EUR",
                    "amount": 900
                }
            }
        });

        let data: NotificationPayloadData = serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(data, NotificationPayloadData::Watchlist { .. }));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_watchlist_payload_data_when_using_camel_case_fields() {
        let json = json!({
            "type": "STATE_CHANGE",
            "oldState": "AVAILABLE",
            "newState": "SOLD",
        });

        let data: WatchlistPayloadData = serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(data, WatchlistPayloadData::StateChange { .. }));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_search_filter_payload_data_when_using_camel_case_fields() {
        let json = json!({
            "userSearchFilterId": "0196580c-e4ca-723f-a7e0-1a73588380f0",
            "userSearchFilterName": "Important Filter",
        });

        let data: SearchFilterPayloadData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_partner_application_payload_data_when_using_camel_case_fields() {
        let json = json!({
            "type": "APPROVED",
            "partnerApplicationId": "0196580c-e4ca-723f-a7e0-1a73588380f0",
        });

        let data: PartnerApplicationPayloadData = serde_json::from_value(json.clone()).unwrap();

        assert!(matches!(
            data,
            PartnerApplicationPayloadData::Approved { .. }
        ));
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_get_notification_data_when_using_camel_case_fields() {
        let json = json!({
            "originEventId": "0196580c-e4ca-723f-a7e0-1a73588380f0",
            "notificationId": "0196580c-e4ca-723f-a7e0-1a73588380f0",
            "payload": {
                "type": "PARTNER_APPLICATION",
                "shopName": "Test Shop",
                "image": "https://test.example/logo.jpg",
                "partnerApplicationPayload": {
                    "type": "REJECTED",
                    "partnerApplicationId": "0196580c-e4ca-723f-a7e0-1a73588380f0"
                }
            },
            "seen": false,
            "external": true,
            "created": "2026-04-22T00:00:00Z",
            "updated": "2026-04-22T01:00:00Z",
        });

        let data: GetNotificationData = serde_json::from_value(json.clone()).unwrap();

        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
