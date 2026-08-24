use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::products::product_data::ProductImageData;
use crate::values::{LocalizedTextData, PriceData};
use axum::response::{IntoResponse, Response};
use localization::{Language, Localized};
use money::Price;
use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange,
        LocalizedProductNotificationSnapshot, PartnerApplicationDecision,
    },
    notification_id::NotificationId,
    notification_kind::NotificationKind,
    presentation::present_image,
};
use notification_service::presentation::NotificationPresentationPreferences;
use notification_service::use_cases::queries::list_notifications::ListedNotification;
use product_listing_core::{product_state::ProductState, title::Title};
use serde::{Deserialize, Serialize, Serializer};
use time::OffsetDateTime;
use url::Url;
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
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(with = "crate::wire::notification_kind")]
    kind: NotificationKind,
    payload: NotificationContentData,
}

impl From<(ListedNotification, NotificationPresentationPreferences)> for NotificationData {
    fn from(
        (value, presentation_preferences): (
            ListedNotification,
            NotificationPresentationPreferences,
        ),
    ) -> Self {
        Self {
            notification_id: Uuid::from(value.notification_id),
            seen: value.seen,
            created: value.created,
            updated: value.updated,
            kind: value.kind,
            payload: (value.content, presentation_preferences).into(),
        }
    }
}

#[derive(Debug)]
enum NotificationContentData {
    Watchlist(WatchlistNotificationPayloadData),
    SearchFilter(SearchFilterNotificationPayloadData),
    PartnerApplication(PartnerApplicationNotificationPayloadData),
}

impl Serialize for NotificationContentData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Watchlist(payload) => payload.serialize(serializer),
            Self::SearchFilter(payload) => payload.serialize(serializer),
            Self::PartnerApplication(payload) => payload.serialize(serializer),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistNotificationPayloadData {
    product_id: Uuid,
    shop_id: Uuid,
    shops_product_id: String,
    shop_slug_id: String,
    product_slug_id: String,
    shop_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductImageData>,
    url: Url,
    view_url: Url,
    change: WatchlistNotificationChangeData,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum WatchlistNotificationChangeData {
    PriceChange {
        old_price: Option<PriceData>,
        new_price: Option<PriceData>,
    },
    StateChange {
        #[serde(with = "crate::wire::product_state")]
        old_state: ProductState,
        #[serde(with = "crate::wire::product_state")]
        new_state: ProductState,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilterNotificationPayloadData {
    product_id: Uuid,
    user_search_filter_id: Uuid,
    user_search_filter_name: String,
    shop_id: Uuid,
    shops_product_id: String,
    shop_slug_id: String,
    product_slug_id: String,
    shop_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductImageData>,
    url: Url,
    view_url: Url,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartnerApplicationNotificationPayloadData {
    partner_shop_application_id: Uuid,
    #[serde(with = "crate::wire::partner_application_decision")]
    decision: PartnerApplicationDecision,
    shop_name: String,
    image: Option<Url>,
}

impl
    From<(
        LocalizedNotificationContent,
        NotificationPresentationPreferences,
    )> for NotificationContentData
{
    fn from(
        (value, presentation_preferences): (
            LocalizedNotificationContent,
            NotificationPresentationPreferences,
        ),
    ) -> Self {
        match value {
            LocalizedNotificationContent::Watchlist {
                product_id,
                snapshot,
                change,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::Watchlist(WatchlistNotificationPayloadData {
                    product_id: Uuid::from(product_id),
                    shop_id: snapshot.shop_id,
                    shops_product_id: snapshot.shops_product_id,
                    shop_slug_id: snapshot.shop_slug_id,
                    product_slug_id: snapshot.product_slug_id,
                    shop_name: snapshot.shop_name,
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                    change: change.into(),
                })
            }
            LocalizedNotificationContent::SearchFilter {
                product_id,
                snapshot,
                user_search_filter_id,
                user_search_filter_name,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::SearchFilter(SearchFilterNotificationPayloadData {
                    product_id: Uuid::from(product_id),
                    user_search_filter_id: Uuid::from(user_search_filter_id),
                    user_search_filter_name: user_search_filter_name.to_string(),
                    shop_id: snapshot.shop_id,
                    shops_product_id: snapshot.shops_product_id,
                    shop_slug_id: snapshot.shop_slug_id,
                    product_slug_id: snapshot.product_slug_id,
                    shop_name: snapshot.shop_name,
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                })
            }
            LocalizedNotificationContent::PartnerApplication {
                partner_shop_application_id,
                snapshot,
                decision,
            } => Self::PartnerApplication(PartnerApplicationNotificationPayloadData {
                partner_shop_application_id: Uuid::from(partner_shop_application_id),
                decision,
                shop_name: snapshot.shop_name.to_string(),
                image: snapshot.image,
            }),
        }
    }
}

fn notification_product_snapshot(
    snapshot: LocalizedProductNotificationSnapshot,
    presentation_preferences: NotificationPresentationPreferences,
) -> NotificationProductSnapshotData {
    NotificationProductSnapshotData {
        shop_id: Uuid::from(snapshot.shop_id),
        shops_product_id: snapshot.shops_product_id.to_string(),
        shop_slug_id: snapshot.shop_slug_id.to_string(),
        product_slug_id: snapshot.product_slug_id.to_string(),
        shop_name: snapshot.shop_name.to_string(),
        title: snapshot.title.map(localized_text_data),
        image: present_image(
            snapshot.image,
            presentation_preferences.prohibited_content_consent,
        )
        .map(ProductImageData::from_presented),
        url: snapshot.url,
        view_url: snapshot.view_url,
    }
}

fn localized_text_data(value: Localized<Language, Title>) -> LocalizedTextData {
    LocalizedTextData {
        text: value.payload.into(),
        language: value.localization,
    }
}

fn price_data(value: Price) -> PriceData {
    PriceData {
        currency: value.currency,
        amount: value.monetary_amount.into(),
    }
}

struct NotificationProductSnapshotData {
    shop_id: Uuid,
    shops_product_id: String,
    shop_slug_id: String,
    product_slug_id: String,
    shop_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductImageData>,
    url: Url,
    view_url: Url,
}

impl From<LocalizedNotificationWatchlistChange> for WatchlistNotificationChangeData {
    fn from(value: LocalizedNotificationWatchlistChange) -> Self {
        match value {
            LocalizedNotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(price_data),
                new_price: new_price.map(price_data),
            },
            LocalizedNotificationWatchlistChange::StateChange {
                old_state,
                new_state,
            } => Self::StateChange {
                old_state,
                new_state,
            },
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
