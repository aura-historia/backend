use crate::core::{notification_id::NotificationId, notification_type::NotificationType};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::user_search_filter_id::UserSearchFilterId;
use common::{
    actor::domain::Actor,
    currency::domain::Currency,
    event_id::EventId,
    language::domain::Language,
    localized::Localized,
    price::domain::{MonetaryAmount, Price},
    product_id::ProductId,
    product_slug_id::ProductSlugId,
    product_state::domain::ProductState,
    shop_id::ShopId,
    shop_name::ShopName,
    shop_slug_id::ShopSlugId,
    shops_product_id::ShopsProductId,
    user_id::UserId,
};
use product::core::product_image::ProductImage;
use product::core::title::Title;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use std::collections::HashMap;
use time::OffsetDateTime;
use tracing::error;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub notification_type: Option<NotificationType>, // None if not yet sent, Some if sent
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub external: bool,
    pub created_by: Actor,
    pub updated_by: Actor,
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
            origin_event_id: self.origin_event_id,
            notification_id: self.notification_id,
            notification_payload: self
                .notification_payload
                .localized(currency, preferred_languages),
            seen: self.seen,
            external: self.external,
            created_by: self.created_by,
            updated_by: self.updated_by,
            created: self.created,
            updated: self.updated,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPayload {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
        shop_name: ShopName,
        title: HashMap<Language, Title>,
        image: Option<ProductImage>,
        url: Url,
        view_url: Url,
        watchlist_payload: NotificationWatchlistPayload,
    },
    SearchFilter {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
        shop_name: ShopName,
        title: HashMap<Language, Title>,
        image: Option<ProductImage>,
        url: Url,
        view_url: Url,
        search_filter_payload: NotificationSearchFilterPayload,
    },
    PartnerApplication {
        shop_name: ShopName,
        image: Option<Url>,
        partner_application_payload: NotificationPartnerApplicationPayload,
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
                image,
                url,
                view_url,
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
                image,
                url,
                view_url,
                watchlist_payload: watchlist_payload.localized(currency),
            },
            NotificationPayload::SearchFilter {
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
            } => LocalizedNotificationPayload::SearchFilter {
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
                image,
                url,
                view_url,
                search_filter_payload,
            },
            NotificationPayload::PartnerApplication {
                shop_name,
                image,
                partner_application_payload,
            } => LocalizedNotificationPayload::PartnerApplication {
                shop_name,
                image,
                partner_application_payload,
            },
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
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

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct NotificationSearchFilterPayload {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_search_filter_name: UserSearchFilterName,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPartnerApplicationPayload {
    Approved {
        partner_application_id: PartnerShopApplicationId,
    },
    Rejected {
        partner_application_id: PartnerShopApplicationId,
    },
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedNotification {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub notification_payload: LocalizedNotificationPayload,
    pub seen: bool,
    pub external: bool,
    pub created_by: Actor,
    pub updated_by: Actor,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_actor_metadata_when_localizing_notification() {
        let notification = Notification {
            user_id: UserId::new(),
            origin_event_id: EventId::new(),
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("Example Shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id: PartnerShopApplicationId::new(),
                },
            },
            seen: false,
            external: false,
            created_by: Actor::System,
            updated_by: Actor::User(UserId::new()),
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let localized = notification
            .clone()
            .localized(&Currency::Eur, &[Language::En]);

        assert_eq!(localized.created_by, notification.created_by);
        assert_eq!(localized.updated_by, notification.updated_by);
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationPayload {
    Watchlist {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
        shop_name: ShopName,
        title: Localized<Language, Title>,
        image: Option<ProductImage>,
        url: Url,
        view_url: Url,
        watchlist_payload: LocalizedNotificationWatchlistPayload,
    },
    SearchFilter {
        product_id: ProductId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
        shop_name: ShopName,
        title: Localized<Language, Title>,
        image: Option<ProductImage>,
        url: Url,
        view_url: Url,
        search_filter_payload: NotificationSearchFilterPayload,
    },
    PartnerApplication {
        shop_name: ShopName,
        image: Option<Url>,
        partner_application_payload: NotificationPartnerApplicationPayload,
    },
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
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
