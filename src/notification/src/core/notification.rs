use crate::core::notification_id::NotificationId;
use common::{
    currency::domain::Currency,
    language::domain::Language,
    localized::Localized,
    price::domain::{MonetaryAmount, Price},
    product_id::ProductId,
    product_state::domain::ProductState,
    shop_id::ShopId,
    shop_name::ShopName,
    shops_product_id::ShopsProductId,
    slug_id::SlugId,
    user_id::UserId,
};
use product::core::title::Title;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::error;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Notification {
    pub fn localized(
        self,
        currency: &Currency,
        preferred_languages: &[Language],
    ) -> LocalizedNotification {
        LocalizedNotification {
            user_id: self.user_id,
            notification_id: self.notification_id,
            notification_payload: self
                .notification_payload
                .localized(currency, preferred_languages),
            seen: self.seen,
            created: self.created,
            updated: self.updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPayload {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: HashMap<Language, Title>,
        watchlist_payload: NotificationWatchlistPayload,
    },
}

impl NotificationPayload {
    pub fn localized(
        self,
        currency: &Currency,
        preferred_languages: &[Language],
    ) -> LocalizedNotificationPayload {
        match self {
            NotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title,
                watchlist_payload,
            } => LocalizedNotificationPayload::Watchlist {
                product_id,
                shop_id,
                shops_product_id,
                shop_slug_id,
                product_slug_id,
                shop_name,
                title: Language::resolve(preferred_languages, title).unwrap_or_else(|| {
                    error!("Failed resolving title. This SHOULD be impossible because the native title always exists.");
                    Localized::new(Language::En, "Unknown title".into())
                }),
                watchlist_payload: watchlist_payload.localized(currency),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationWatchlistPayload {
    PriceChange {
        old_price: HashMap<Currency, MonetaryAmount>,
        new_price: HashMap<Currency, MonetaryAmount>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}

impl NotificationWatchlistPayload {
    pub fn localized(self, currency: &Currency) -> LocalizedNotificationWatchlistPayload {
        match self {
            NotificationWatchlistPayload::PriceChange {
                old_price,
                new_price,
            } => LocalizedNotificationWatchlistPayload::PriceChange {
                old_price: Currency::resolve(&[*currency], old_price),
                new_price: Currency::resolve(&[*currency], new_price),
            },
            NotificationWatchlistPayload::StateChange {
                old_state,
                new_state,
            } => LocalizedNotificationWatchlistPayload::StateChange {
                old_state,
                new_state,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedNotification {
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_payload: LocalizedNotificationPayload,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationPayload {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: SlugId<0>,
        product_slug_id: SlugId<6>,
        shop_name: ShopName,
        title: Localized<Language, Title>,
        watchlist_payload: LocalizedNotificationWatchlistPayload,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationWatchlistPayload {
    PriceChange {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}
