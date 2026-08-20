use crate::{notification_id::NotificationId, notification_type::NotificationType};
use common::partner_shop_application_id::PartnerShopApplicationId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use common::{event_id::EventId, user_id::UserId};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_core::{
    product_id::ProductId, product_image::ProductImage, product_slug_id::ProductSlugId,
    product_state::ProductState, shops_product_id::ShopsProductId, title::Title,
};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    user_id: UserId,
    origin_event_id: EventId,
    notification_id: NotificationId,
    notification_type: Option<NotificationType>, // None if not yet sent, Some if sent
    notification_payload: NotificationPayload,
    seen: bool,
    external: bool,
}

impl Notification {
    pub fn new(
        user_id: UserId,
        origin_event_id: EventId,
        notification_payload: NotificationPayload,
        external: bool,
    ) -> Self {
        Self {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload,
            seen: false,
            external,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedNotificationState) -> Self {
        Self {
            user_id: state.user_id,
            origin_event_id: state.origin_event_id,
            notification_id: state.notification_id,
            notification_type: state.notification_type,
            notification_payload: state.notification_payload,
            seen: state.seen,
            external: state.external,
        }
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn origin_event_id(&self) -> EventId {
        self.origin_event_id
    }

    pub fn notification_id(&self) -> NotificationId {
        self.notification_id
    }

    pub fn notification_type(&self) -> Option<NotificationType> {
        self.notification_type
    }

    pub fn notification_payload(&self) -> &NotificationPayload {
        &self.notification_payload
    }

    pub fn seen(&self) -> bool {
        self.seen
    }

    pub fn external(&self) -> bool {
        self.external
    }

    pub fn mark_seen(&mut self, seen: bool) {
        self.seen = seen;
    }

    pub fn mark_sent_as(&mut self, notification_type: NotificationType) {
        self.notification_type = Some(notification_type);
    }
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedNotificationState {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub notification_type: Option<NotificationType>,
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub external: bool,
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
        title: Option<HashMap<Language, Title>>,
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
        title: Option<HashMap<Language, Title>>,
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
                title: title.and_then(|title| Language::resolve(preferred_languages, title)),
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
                title: title.and_then(|title| Language::resolve(preferred_languages, title)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_url(path: &str) -> Url {
        match Url::parse(&format!("https://example.test/{path}")) {
            Ok(url) => url,
            Err(error) => panic!("test URL must parse: {error}"),
        }
    }

    fn titles() -> HashMap<Language, Title> {
        HashMap::from([
            (Language::En, Title::from("English title")),
            (Language::De, Title::from("Deutscher Titel")),
        ])
    }

    fn base_notification(notification_payload: NotificationPayload) -> Notification {
        Notification::new(UserId::new(), EventId::new(), notification_payload, false)
    }

    fn partner_application_payload() -> NotificationPayload {
        NotificationPayload::PartnerApplication {
            shop_name: ShopName::from("Example Shop"),
            image: None,
            partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                partner_application_id: PartnerShopApplicationId::new(),
            },
        }
    }

    #[test]
    fn should_create_notification_with_defaults() {
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        let notification_payload = partner_application_payload();

        let notification =
            Notification::new(user_id, origin_event_id, notification_payload.clone(), true);

        assert_eq!(notification.user_id(), user_id);
        assert_eq!(notification.origin_event_id(), origin_event_id);
        assert_eq!(notification.notification_type(), None);
        assert_eq!(notification.notification_payload(), &notification_payload);
        assert!(!notification.seen());
        assert!(notification.external());
    }

    #[test]
    fn should_mark_notification_seen() {
        let mut notification = base_notification(partner_application_payload());

        notification.mark_seen(true);

        assert!(notification.seen());
    }

    #[test]
    fn should_mark_notification_sent_as_type() {
        let mut notification = base_notification(partner_application_payload());

        notification.mark_sent_as(NotificationType::Email);

        assert_eq!(
            notification.notification_type(),
            Some(NotificationType::Email)
        );
    }

    #[test]
    fn should_rehydrate_notification() {
        let notification_payload = partner_application_payload();
        let state = RehydratedNotificationState {
            user_id: UserId::new(),
            origin_event_id: EventId::new(),
            notification_id: NotificationId::new(),
            notification_type: Some(NotificationType::Email),
            notification_payload: notification_payload.clone(),
            seen: true,
            external: true,
        };

        let notification = Notification::rehydrate(state.clone());

        assert_eq!(notification.user_id(), state.user_id);
        assert_eq!(notification.origin_event_id(), state.origin_event_id);
        assert_eq!(notification.notification_id(), state.notification_id);
        assert_eq!(notification.notification_type(), state.notification_type);
        assert_eq!(notification.notification_payload(), &notification_payload);
        assert!(notification.seen());
        assert!(notification.external());
    }

    #[test]
    fn should_localize_watchlist_price_change_with_requested_language_and_currency() {
        let notification = base_notification(NotificationPayload::Watchlist {
            product_id: ProductId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            product_slug_id: ProductSlugId::from("product"),
            shop_name: ShopName::from("Example Shop"),
            title: Some(titles()),
            image: None,
            url: test_url("source"),
            view_url: test_url("view"),
            watchlist_payload: NotificationWatchlistPayload::PriceChange {
                old_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(100_u32))]),
                new_price: HashMap::from([(Currency::Eur, MonetaryAmount::from(80_u32))]),
            },
        });

        let localized = notification
            .notification_payload()
            .clone()
            .localized(&Currency::Eur, &[Language::De]);

        match localized {
            LocalizedNotificationPayload::Watchlist {
                title,
                watchlist_payload:
                    LocalizedNotificationWatchlistPayload::PriceChange {
                        old_price,
                        new_price,
                    },
                ..
            } => {
                let title = title.expect("title should be present");
                assert_eq!(title.localization, Language::De);
                assert_eq!(
                    old_price,
                    Some(Price::new(MonetaryAmount::from(100_u32), Currency::Eur))
                );
                assert_eq!(
                    new_price,
                    Some(Price::new(MonetaryAmount::from(80_u32), Currency::Eur))
                );
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn should_localize_watchlist_state_change() {
        let notification = base_notification(NotificationPayload::Watchlist {
            product_id: ProductId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            product_slug_id: ProductSlugId::from("product"),
            shop_name: ShopName::from("Example Shop"),
            title: Some(titles()),
            image: None,
            url: test_url("source"),
            view_url: test_url("view"),
            watchlist_payload: NotificationWatchlistPayload::StateChange {
                old_state: ProductState::Available,
                new_state: ProductState::Sold,
            },
        });

        let localized = notification
            .notification_payload()
            .clone()
            .localized(&Currency::Eur, &[Language::En]);

        match localized {
            LocalizedNotificationPayload::Watchlist {
                watchlist_payload:
                    LocalizedNotificationWatchlistPayload::StateChange {
                        old_state,
                        new_state,
                    },
                ..
            } => {
                assert_eq!(old_state, ProductState::Available);
                assert_eq!(new_state, ProductState::Sold);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn should_localize_without_title() {
        let notification = base_notification(NotificationPayload::Watchlist {
            product_id: ProductId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            product_slug_id: ProductSlugId::from("product"),
            shop_name: ShopName::from("Example Shop"),
            title: None,
            image: None,
            url: test_url("source"),
            view_url: test_url("view"),
            watchlist_payload: NotificationWatchlistPayload::StateChange {
                old_state: ProductState::Available,
                new_state: ProductState::Sold,
            },
        });

        let localized = notification
            .notification_payload()
            .clone()
            .localized(&Currency::Eur, &[Language::En]);

        match localized {
            LocalizedNotificationPayload::Watchlist { title, .. } => assert!(title.is_none()),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn should_localize_search_filter_payload() {
        let payload = NotificationSearchFilterPayload {
            user_search_filter_id: UserSearchFilterId::new(),
            user_search_filter_name: UserSearchFilterName::from("Deals"),
        };
        let notification = base_notification(NotificationPayload::SearchFilter {
            product_id: ProductId::new(),
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            product_slug_id: ProductSlugId::from("product"),
            shop_name: ShopName::from("Example Shop"),
            title: Some(titles()),
            image: None,
            url: test_url("source"),
            view_url: test_url("view"),
            search_filter_payload: payload.clone(),
        });

        let localized = notification
            .notification_payload()
            .clone()
            .localized(&Currency::Eur, &[Language::De]);

        match localized {
            LocalizedNotificationPayload::SearchFilter {
                title,
                search_filter_payload,
                ..
            } => {
                let title = title.expect("title should be present");
                assert_eq!(title.localization, Language::De);
                assert_eq!(search_filter_payload, payload);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn should_localize_partner_application_payload() {
        let payload = NotificationPartnerApplicationPayload::Rejected {
            partner_application_id: PartnerShopApplicationId::new(),
        };
        let notification = base_notification(NotificationPayload::PartnerApplication {
            shop_name: ShopName::from("Example Shop"),
            image: Some(test_url("shop.png")),
            partner_application_payload: payload.clone(),
        });

        let localized = notification
            .notification_payload()
            .clone()
            .localized(&Currency::Eur, &[Language::En]);

        match localized {
            LocalizedNotificationPayload::PartnerApplication {
                shop_name,
                image,
                partner_application_payload,
            } => {
                assert_eq!(shop_name, ShopName::from("Example Shop"));
                assert_eq!(image, Some(test_url("shop.png")));
                assert_eq!(partner_application_payload, payload);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
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
        title: Option<Localized<Language, Title>>,
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
        title: Option<Localized<Language, Title>>,
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
