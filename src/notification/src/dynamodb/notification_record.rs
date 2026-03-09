use crate::{
    core::notification_id::NotificationId,
    dynamodb::{
        notification_medium_record::NotificationMediumRecord,
        notification_reason_record::NotificationReasonRecord,
    },
};
use common::{
    price::record::PriceRecord, product_id::ProductId, shop_id::ShopId,
    shops_product_id::ShopsProductId, slug_id::SlugId, user_id::UserId,
};
use product::dynamodb::{
    product_image_record::ProductImageRecord, product_state_record::ProductStateRecord,
};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;

// #[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct NotificationRecord {
    pub pk: String,
    pub sk: String,
    pub lsi1_sk: String,
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_type: NotificationMediumRecord,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notification_reason: Option<NotificationReasonRecord>,
    pub seen: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<ProductImageRecord>,

    // watchlist
    // product
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_id: Option<ProductId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub product_slug_id: Option<SlugId<6>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_slug_id: Option<SlugId<0>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_id: Option<ShopId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shops_product_id: Option<ShopsProductId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_it: Option<String>,
    // price-change
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_price_nzd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_native: Option<PriceRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_eur: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_usd: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_gbp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_aud: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_cad: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_price_nzd: Option<u64>,
    // state-change
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub new_state: Option<ProductStateRecord>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub old_state: Option<ProductStateRecord>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(notification_id: &NotificationId) -> String {
    format!("user#notification#{notification_id}")
}

pub fn mk_lsi1_sk(
    notification_id: &NotificationId,
    notification_reason: &NotificationReasonRecord,
) -> String {
    let format_watchlist: fn(&NotificationId) -> String =
        |notification_id| format!("user#notification#watchlist#{notification_id}");
    match notification_reason {
        NotificationReasonRecord::WatchlistStateListed => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateAvailable => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateReserved => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateSold => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateRemoved => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistStateUnknown => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceDiscovered => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceDropped => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceIncreased => format_watchlist(notification_id),
        NotificationReasonRecord::WatchlistPriceRemoved => format_watchlist(notification_id),
    }
}
