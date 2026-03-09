use crate::core::{notification_id::NotificationId, notification_medium::NotificationMedium};
use common::{
    language::domain::Language, price::domain::Price, product_id::ProductId,
    product_state::domain::ProductState, shop_id::ShopId, shop_name::ShopName, slug_id::SlugId,
    user_id::UserId,
};
use product::core::title::Title;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_medium: Option<NotificationMedium>,
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPayload {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: String,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: HashMap<Language, Title>,
        watchlist_payload: NotificationWatchlistPayload,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationWatchlistPayload {
    PriceChange {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}
