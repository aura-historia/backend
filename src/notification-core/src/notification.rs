use crate::{notification_id::NotificationId, notification_kind::NotificationKind};
use domain_primitives::event_id::EventId;
use localization::Localized;
use money::Price;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId, product_state::ProductState,
    shop_listing_id::ShopListingId, title::Title,
};
use search_filter_core::{
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use std::collections::HashMap;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    notification_id: NotificationId,
    user_id: UserId,
    content: NotificationContent,
    seen: bool,
}

impl Notification {
    pub fn new(
        notification_id: NotificationId,
        user_id: UserId,
        content: NotificationContent,
    ) -> Self {
        Self {
            notification_id,
            user_id,
            content,
            seen: false,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedNotificationState,
    ) -> Result<Self, RehydrateNotificationError> {
        Ok(Self {
            notification_id: state.notification_id,
            user_id: state.user_id,
            content: state.content,
            seen: state.seen,
        })
    }

    pub fn notification_id(&self) -> NotificationId {
        self.notification_id
    }
    pub fn user_id(&self) -> UserId {
        self.user_id
    }
    pub fn content(&self) -> &NotificationContent {
        &self.content
    }
    pub fn kind(&self) -> NotificationKind {
        self.content.kind()
    }
    pub fn seen(&self) -> bool {
        self.seen
    }
    pub fn origin_event_id(&self) -> Option<EventId> {
        self.content.origin_event_id()
    }
    pub fn product_listing_id(&self) -> Option<ProductListingId> {
        self.content.product_listing_id()
    }
    pub fn mark_seen(&mut self, seen: bool) {
        self.seen = seen;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("persisted notification state is invalid")]
pub struct RehydrateNotificationError;

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedNotificationState {
    pub notification_id: NotificationId,
    pub user_id: UserId,
    pub content: NotificationContent,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationContent {
    Watchlist {
        origin_event_id: EventId,
        product_listing_id: ProductListingId,
        snapshot: ProductListingNotificationSnapshot,
        change: NotificationWatchlistChange,
    },
    SearchFilter {
        origin_event_id: EventId,
        product_listing_id: ProductListingId,
        user_search_filter_id: UserSearchFilterId,
        snapshot: ProductListingNotificationSnapshot,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnerApplication {
        partner_shop_application_id: PartnerShopApplicationId,
        snapshot: PartnerApplicationNotificationSnapshot,
        decision: PartnerApplicationDecision,
    },
}

impl NotificationContent {
    pub fn kind(&self) -> NotificationKind {
        match self {
            Self::Watchlist {
                change: NotificationWatchlistChange::PriceChange { .. },
                ..
            } => NotificationKind::WatchlistPriceChanged,
            Self::Watchlist {
                change: NotificationWatchlistChange::StateChange { .. },
                ..
            } => NotificationKind::WatchlistStateChanged,
            Self::SearchFilter { .. } => NotificationKind::SearchFilterMatch,
            Self::PartnerApplication {
                decision: PartnerApplicationDecision::Approved,
                ..
            } => NotificationKind::PartnerApplicationApproved,
            Self::PartnerApplication {
                decision: PartnerApplicationDecision::Rejected,
                ..
            } => NotificationKind::PartnerApplicationRejected,
        }
    }

    pub fn origin_event_id(&self) -> Option<EventId> {
        match self {
            Self::Watchlist {
                origin_event_id, ..
            }
            | Self::SearchFilter {
                origin_event_id, ..
            } => Some(*origin_event_id),
            Self::PartnerApplication { .. } => None,
        }
    }

    pub fn product_listing_id(&self) -> Option<ProductListingId> {
        match self {
            Self::Watchlist {
                product_listing_id, ..
            }
            | Self::SearchFilter {
                product_listing_id, ..
            } => Some(*product_listing_id),
            Self::PartnerApplication { .. } => None,
        }
    }

    pub fn localized(
        self,
        preferred_languages: &[localization::Language],
    ) -> LocalizedNotificationContent {
        match self {
            Self::Watchlist {
                product_listing_id,
                snapshot,
                change,
                ..
            } => LocalizedNotificationContent::Watchlist {
                product_listing_id,
                snapshot: snapshot.localized(preferred_languages),
                change: change.localized(),
            },
            Self::SearchFilter {
                product_listing_id,
                snapshot,
                user_search_filter_id,
                user_search_filter_name,
                ..
            } => LocalizedNotificationContent::SearchFilter {
                product_listing_id,
                snapshot: snapshot.localized(preferred_languages),
                user_search_filter_id,
                user_search_filter_name,
            },
            Self::PartnerApplication {
                partner_shop_application_id,
                snapshot,
                decision,
            } => LocalizedNotificationContent::PartnerApplication {
                partner_shop_application_id,
                snapshot,
                decision,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingNotificationSnapshot {
    pub shop_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_slug_id: ShopSlugId,
    pub product_listing_slug_id: ProductListingSlugId,
    pub shop_name: ShopName,
    pub title: Option<HashMap<localization::Language, Title>>,
    pub image: Option<ProductListingImage>,
    pub url: Url,
    pub view_url: Url,
}

impl ProductListingNotificationSnapshot {
    fn localized(
        self,
        preferred_languages: &[localization::Language],
    ) -> LocalizedProductListingNotificationSnapshot {
        LocalizedProductListingNotificationSnapshot {
            shop_id: self.shop_id,
            shop_listing_id: self.shop_listing_id,
            shop_slug_id: self.shop_slug_id,
            product_listing_slug_id: self.product_listing_slug_id,
            shop_name: self.shop_name,
            title: self
                .title
                .and_then(|titles| localization::Language::resolve(preferred_languages, titles)),
            image: self.image,
            url: self.url,
            view_url: self.view_url,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerApplicationNotificationSnapshot {
    pub shop_name: ShopName,
    pub image: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerApplicationDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationWatchlistChange {
    PriceChange {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}

impl NotificationWatchlistChange {
    fn localized(self) -> LocalizedNotificationWatchlistChange {
        match self {
            Self::PriceChange {
                old_price,
                new_price,
            } => LocalizedNotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            },
            Self::StateChange {
                old_state,
                new_state,
            } => LocalizedNotificationWatchlistChange::StateChange {
                old_state,
                new_state,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationContent {
    Watchlist {
        product_listing_id: ProductListingId,
        snapshot: LocalizedProductListingNotificationSnapshot,
        change: LocalizedNotificationWatchlistChange,
    },
    SearchFilter {
        product_listing_id: ProductListingId,
        snapshot: LocalizedProductListingNotificationSnapshot,
        user_search_filter_id: UserSearchFilterId,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnerApplication {
        partner_shop_application_id: PartnerShopApplicationId,
        snapshot: PartnerApplicationNotificationSnapshot,
        decision: PartnerApplicationDecision,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductListingNotificationSnapshot {
    pub shop_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_slug_id: ShopSlugId,
    pub product_listing_slug_id: ProductListingSlugId,
    pub shop_name: ShopName,
    pub title: Option<Localized<localization::Language, Title>>,
    pub image: Option<ProductListingImage>,
    pub url: Url,
    pub view_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationWatchlistChange {
    PriceChange {
        old_price: Option<Price>,
        new_price: Option<Price>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}
