use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use money::{Currency, MonetaryAmount, Price};
use notification_core::{
    notification::{
        Notification, NotificationContent, NotificationWatchlistChange, PartnerApplicationDecision,
        PartnerApplicationNotificationSnapshot, ProductListingNotificationSnapshot,
        RehydratedNotificationState,
    },
    notification_id::NotificationId,
    notification_kind::NotificationKind,
};
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId, shop_listing_id::ShopListingId,
};
use product_listing_core::{product_listing_image::ProductListingImage, title::Title};
use search_filter_core::{
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use serde::{Deserialize, Serialize};
use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use std::collections::{HashMap, HashSet};
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

pub(crate) const PAYLOAD_VERSION: i16 = 1;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct NotificationRow {
    pub(crate) notification_id: uuid::Uuid,
    pub(crate) user_id: uuid::Uuid,
    pub(crate) kind: String,
    pub(crate) origin_event_id: Option<uuid::Uuid>,
    pub(crate) product_listing_id: Option<uuid::Uuid>,
    pub(crate) user_search_filter_id: Option<uuid::Uuid>,
    pub(crate) partner_shop_application_id: Option<uuid::Uuid>,
    pub(crate) payload_version: i16,
    pub(crate) payload: serde_json::Value,
    pub(crate) seen: bool,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NotificationMappingError {
    #[error("unknown notification kind {0}")]
    UnknownKind(String),
    #[error("unknown notification title language {0}")]
    UnknownLanguage(String),
    #[error("notification title contains duplicate language {0}")]
    DuplicateTitleLanguage(String),
    #[error("unsupported notification payload version {0}")]
    UnsupportedPayloadVersion(i16),
    #[error("notification payload serialization failed")]
    PayloadSerialization(#[source] serde_json::Error),
    #[error("notification payload is invalid")]
    InvalidPayload(#[source] serde_json::Error),
    #[error("notification source columns do not match its kind")]
    SourceShapeMismatch,
    #[error("notification kind does not match its payload")]
    KindPayloadMismatch,
    #[error("notification rehydration failed")]
    Rehydrate(#[source] notification_core::notification::RehydrateNotificationError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum NotificationPayloadV1 {
    Watchlist {
        snapshot: ProductListingNotificationSnapshotV1,
        change: NotificationWatchlistChangeV1,
    },
    SearchFilter {
        snapshot: ProductListingNotificationSnapshotV1,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnerApplication {
        snapshot: PartnerApplicationNotificationSnapshotV1,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalizedTitleV1 {
    language: String,
    title: Title,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PersistedCurrency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PersistedPrice {
    currency: PersistedCurrency,
    amount: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingNotificationSnapshotV1 {
    shop_id: ShopId,
    shop_listing_id: ShopListingId,
    shop_slug_id: ShopSlugId,
    product_listing_slug_id: ProductListingSlugId,
    shop_name: ShopName,
    title: Option<Vec<LocalizedTitleV1>>,
    image: Option<ProductListingImage>,
    url: Url,
    view_url: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum NotificationWatchlistChangeV1 {
    PriceChange {
        old_price: Option<PersistedPrice>,
        new_price: Option<PersistedPrice>,
    },
    #[serde(rename = "STATE_CHANGE")]
    AvailabilityChange {
        #[serde(
            rename = "old_state",
            serialize_with = "serialize_listing_availability",
            deserialize_with = "deserialize_listing_availability"
        )]
        old_availability: ListingAvailability,
        #[serde(
            rename = "new_state",
            serialize_with = "serialize_listing_availability",
            deserialize_with = "deserialize_listing_availability"
        )]
        new_availability: ListingAvailability,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartnerApplicationNotificationSnapshotV1 {
    shop_name: ShopName,
    image: Option<Url>,
}

impl From<&ProductListingNotificationSnapshot> for ProductListingNotificationSnapshotV1 {
    fn from(snapshot: &ProductListingNotificationSnapshot) -> Self {
        Self {
            shop_id: snapshot.shop_id,
            shop_listing_id: snapshot.shop_listing_id.clone(),
            shop_slug_id: snapshot.shop_slug_id.clone(),
            product_listing_slug_id: snapshot.product_listing_slug_id.clone(),
            shop_name: snapshot.shop_name.clone(),
            title: snapshot.title.as_ref().map(|titles| {
                titles
                    .iter()
                    .map(|(language, title)| LocalizedTitleV1 {
                        language: language.as_str().to_owned(),
                        title: title.clone(),
                    })
                    .collect()
            }),
            image: snapshot.image.clone(),
            url: snapshot.url.clone(),
            view_url: snapshot.view_url.clone(),
        }
    }
}

impl TryFrom<ProductListingNotificationSnapshotV1> for ProductListingNotificationSnapshot {
    type Error = NotificationMappingError;

    fn try_from(snapshot: ProductListingNotificationSnapshotV1) -> Result<Self, Self::Error> {
        let title = snapshot
            .title
            .map(|titles| {
                let mut seen_languages = HashSet::new();
                titles
                    .into_iter()
                    .map(|title| {
                        let language = parse_language(&title.language)?;
                        if !seen_languages.insert(language) {
                            return Err(NotificationMappingError::DuplicateTitleLanguage(
                                title.language,
                            ));
                        }
                        Ok((language, title.title))
                    })
                    .collect::<Result<HashMap<_, _>, NotificationMappingError>>()
            })
            .transpose()?;
        Ok(Self {
            shop_id: snapshot.shop_id,
            shop_listing_id: snapshot.shop_listing_id,
            shop_slug_id: snapshot.shop_slug_id,
            product_listing_slug_id: snapshot.product_listing_slug_id,
            shop_name: snapshot.shop_name,
            title,
            image: snapshot.image,
            url: snapshot.url,
            view_url: snapshot.view_url,
        })
    }
}

impl From<&NotificationWatchlistChange> for NotificationWatchlistChangeV1 {
    fn from(change: &NotificationWatchlistChange) -> Self {
        match change {
            NotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(price_data_from_price),
                new_price: new_price.map(price_data_from_price),
            },
            NotificationWatchlistChange::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability: *old_availability,
                new_availability: *new_availability,
            },
        }
    }
}

impl From<NotificationWatchlistChangeV1> for NotificationWatchlistChange {
    fn from(change: NotificationWatchlistChangeV1) -> Self {
        match change {
            NotificationWatchlistChangeV1::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(price_from_data),
                new_price: new_price.map(price_from_data),
            },
            NotificationWatchlistChangeV1::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability,
                new_availability,
            },
        }
    }
}

impl From<&PartnerApplicationNotificationSnapshot> for PartnerApplicationNotificationSnapshotV1 {
    fn from(snapshot: &PartnerApplicationNotificationSnapshot) -> Self {
        Self {
            shop_name: snapshot.shop_name.clone(),
            image: snapshot.image.clone(),
        }
    }
}

impl From<PartnerApplicationNotificationSnapshotV1> for PartnerApplicationNotificationSnapshot {
    fn from(snapshot: PartnerApplicationNotificationSnapshotV1) -> Self {
        Self {
            shop_name: snapshot.shop_name,
            image: snapshot.image,
        }
    }
}

pub(crate) struct NotificationWriteValues {
    pub(crate) notification_id: uuid::Uuid,
    pub(crate) user_id: uuid::Uuid,
    pub(crate) kind: &'static str,
    pub(crate) origin_event_id: Option<uuid::Uuid>,
    pub(crate) product_listing_id: Option<uuid::Uuid>,
    pub(crate) user_search_filter_id: Option<uuid::Uuid>,
    pub(crate) partner_shop_application_id: Option<uuid::Uuid>,
    pub(crate) payload: serde_json::Value,
}

impl TryFrom<&Notification> for NotificationWriteValues {
    type Error = NotificationMappingError;

    fn try_from(notification: &Notification) -> Result<Self, Self::Error> {
        let (
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partner_shop_application_id,
            payload,
        ) = match notification.content() {
            NotificationContent::Watchlist {
                origin_event_id,
                product_listing_id,
                snapshot,
                change,
            } => (
                Some(uuid::Uuid::from(*origin_event_id)),
                Some(uuid::Uuid::from(*product_listing_id)),
                None,
                None,
                NotificationPayloadV1::Watchlist {
                    snapshot: snapshot.into(),
                    change: change.into(),
                },
            ),
            NotificationContent::SearchFilter {
                origin_event_id,
                product_listing_id,
                user_search_filter_id,
                snapshot,
                user_search_filter_name,
            } => (
                Some(uuid::Uuid::from(*origin_event_id)),
                Some(uuid::Uuid::from(*product_listing_id)),
                Some(uuid::Uuid::from(*user_search_filter_id)),
                None,
                NotificationPayloadV1::SearchFilter {
                    snapshot: snapshot.into(),
                    user_search_filter_name: user_search_filter_name.clone(),
                },
            ),
            NotificationContent::PartnerApplication {
                partner_shop_application_id,
                snapshot,
                ..
            } => (
                None,
                None,
                None,
                Some(uuid::Uuid::from(*partner_shop_application_id)),
                NotificationPayloadV1::PartnerApplication {
                    snapshot: snapshot.into(),
                },
            ),
        };
        let payload = serde_json::to_value(payload)
            .map_err(NotificationMappingError::PayloadSerialization)?;
        Ok(Self {
            notification_id: uuid::Uuid::from(notification.notification_id()),
            user_id: uuid::Uuid::from(notification.user_id()),
            kind: notification.kind().as_str(),
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partner_shop_application_id,
            payload,
        })
    }
}

impl TryFrom<NotificationRow> for Notification {
    type Error = NotificationMappingError;

    fn try_from(row: NotificationRow) -> Result<Self, Self::Error> {
        if row.payload_version != PAYLOAD_VERSION {
            return Err(NotificationMappingError::UnsupportedPayloadVersion(
                row.payload_version,
            ));
        }
        let kind = parse_kind(&row.kind)?;
        let payload = serde_json::from_value::<NotificationPayloadV1>(row.payload)
            .map_err(NotificationMappingError::InvalidPayload)?;
        let content = match (
            kind,
            payload,
            row.origin_event_id,
            row.product_listing_id,
            row.user_search_filter_id,
            row.partner_shop_application_id,
        ) {
            (
                NotificationKind::WatchlistPriceChanged | NotificationKind::WatchlistStateChanged,
                NotificationPayloadV1::Watchlist { snapshot, change },
                Some(origin_event_id),
                Some(product_listing_id),
                None,
                None,
            ) => {
                let change: NotificationWatchlistChange = change.into();
                if change_kind(&change) != kind {
                    return Err(NotificationMappingError::KindPayloadMismatch);
                }
                NotificationContent::Watchlist {
                    origin_event_id: EventId::from(origin_event_id),
                    product_listing_id: ProductListingId::from(product_listing_id),
                    snapshot: snapshot.try_into()?,
                    change,
                }
            }
            (
                NotificationKind::SearchFilterMatch,
                NotificationPayloadV1::SearchFilter {
                    snapshot,
                    user_search_filter_name,
                },
                Some(origin_event_id),
                Some(product_listing_id),
                Some(user_search_filter_id),
                None,
            ) => NotificationContent::SearchFilter {
                origin_event_id: EventId::from(origin_event_id),
                product_listing_id: ProductListingId::from(product_listing_id),
                user_search_filter_id: UserSearchFilterId::from(user_search_filter_id),
                snapshot: snapshot.try_into()?,
                user_search_filter_name,
            },
            (
                NotificationKind::PartnerApplicationApproved
                | NotificationKind::PartnerApplicationRejected,
                NotificationPayloadV1::PartnerApplication { snapshot },
                None,
                None,
                None,
                Some(partner_shop_application_id),
            ) => NotificationContent::PartnerApplication {
                partner_shop_application_id: PartnerShopApplicationId::from(
                    partner_shop_application_id,
                ),
                snapshot: snapshot.into(),
                decision: if kind == NotificationKind::PartnerApplicationApproved {
                    PartnerApplicationDecision::Approved
                } else {
                    PartnerApplicationDecision::Rejected
                },
            },
            (_, NotificationPayloadV1::Watchlist { .. }, _, _, _, _)
            | (_, NotificationPayloadV1::SearchFilter { .. }, _, _, _, _)
            | (_, NotificationPayloadV1::PartnerApplication { .. }, _, _, _, _) => {
                return Err(NotificationMappingError::SourceShapeMismatch);
            }
        };
        Notification::rehydrate(RehydratedNotificationState {
            notification_id: NotificationId::from(row.notification_id),
            user_id: UserId::from(row.user_id),
            content,
            seen: row.seen,
        })
        .map_err(NotificationMappingError::Rehydrate)
    }
}

fn price_data_from_price(price: Price) -> PersistedPrice {
    let currency = match price.currency {
        Currency::Eur => PersistedCurrency::Eur,
        Currency::Gbp => PersistedCurrency::Gbp,
        Currency::Usd => PersistedCurrency::Usd,
        Currency::Aud => PersistedCurrency::Aud,
        Currency::Cad => PersistedCurrency::Cad,
        Currency::Nzd => PersistedCurrency::Nzd,
        Currency::Cny => PersistedCurrency::Cny,
        Currency::Brl => PersistedCurrency::Brl,
        Currency::Pln => PersistedCurrency::Pln,
        Currency::Try => PersistedCurrency::Try,
        Currency::Jpy => PersistedCurrency::Jpy,
        Currency::Czk => PersistedCurrency::Czk,
        Currency::Rub => PersistedCurrency::Rub,
        Currency::Aed => PersistedCurrency::Aed,
        Currency::Sar => PersistedCurrency::Sar,
        Currency::Hkd => PersistedCurrency::Hkd,
        Currency::Sgd => PersistedCurrency::Sgd,
        Currency::Chf => PersistedCurrency::Chf,
    };
    PersistedPrice {
        currency,
        amount: price.monetary_amount.into(),
    }
}

fn price_from_data(price: PersistedPrice) -> Price {
    let currency = match price.currency {
        PersistedCurrency::Eur => Currency::Eur,
        PersistedCurrency::Gbp => Currency::Gbp,
        PersistedCurrency::Usd => Currency::Usd,
        PersistedCurrency::Aud => Currency::Aud,
        PersistedCurrency::Cad => Currency::Cad,
        PersistedCurrency::Nzd => Currency::Nzd,
        PersistedCurrency::Cny => Currency::Cny,
        PersistedCurrency::Brl => Currency::Brl,
        PersistedCurrency::Pln => Currency::Pln,
        PersistedCurrency::Try => Currency::Try,
        PersistedCurrency::Jpy => Currency::Jpy,
        PersistedCurrency::Czk => Currency::Czk,
        PersistedCurrency::Rub => Currency::Rub,
        PersistedCurrency::Aed => Currency::Aed,
        PersistedCurrency::Sar => Currency::Sar,
        PersistedCurrency::Hkd => Currency::Hkd,
        PersistedCurrency::Sgd => Currency::Sgd,
        PersistedCurrency::Chf => Currency::Chf,
    };
    Price::new(MonetaryAmount::from(price.amount), currency)
}

fn serialize_listing_availability<S>(
    availability: &ListingAvailability,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(availability.as_str())
}

fn deserialize_listing_availability<'de, D>(
    deserializer: D,
) -> Result<ListingAvailability, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    ListingAvailability::from_code(&value).ok_or_else(|| {
        <D::Error as serde::de::Error>::custom(format!("unknown listing availability {value}"))
    })
}

fn parse_language(value: &str) -> Result<localization::Language, NotificationMappingError> {
    localization::Language::from_code(value)
        .ok_or_else(|| NotificationMappingError::UnknownLanguage(value.to_owned()))
}

fn parse_kind(value: &str) -> Result<NotificationKind, NotificationMappingError> {
    NotificationKind::iter()
        .find(|kind| kind.as_str() == value)
        .ok_or_else(|| NotificationMappingError::UnknownKind(value.to_owned()))
}

fn change_kind(change: &NotificationWatchlistChange) -> NotificationKind {
    match change {
        NotificationWatchlistChange::PriceChange { .. } => NotificationKind::WatchlistPriceChanged,
        NotificationWatchlistChange::AvailabilityChange { .. } => {
            NotificationKind::WatchlistStateChanged
        }
    }
}

pub(crate) fn mapping_error(error: NotificationMappingError) -> BoxError {
    box_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::{Currency, MonetaryAmount};

    #[test]
    fn should_parse_each_canonical_persisted_kind() {
        for expected in NotificationKind::iter() {
            assert!(matches!(
                parse_kind(expected.as_str()),
                Ok(actual) if actual == expected
            ));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_persisted_kind() {
        assert!(matches!(
            parse_kind("watchlist_price_changed"),
            Err(NotificationMappingError::UnknownKind(value)) if value == "watchlist_price_changed"
        ));
    }

    #[test]
    fn should_serialize_canonical_listing_availability_in_legacy_v1_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let change = NotificationWatchlistChange::AvailabilityChange {
            old_availability: ListingAvailability::Available,
            new_availability: ListingAvailability::InStock,
        };
        let persisted = NotificationWatchlistChangeV1::from(&change);

        assert_eq!(
            serde_json::json!({
                "type": "STATE_CHANGE",
                "old_state": "AVAILABLE",
                "new_state": "IN_STOCK",
            }),
            serde_json::to_value(persisted)?
        );
        Ok(())
    }

    #[test]
    fn should_reject_unknown_listing_availability_in_legacy_v1_shape() {
        assert!(
            serde_json::from_value::<NotificationWatchlistChangeV1>(serde_json::json!({
                "type": "STATE_CHANGE",
                "old_state": "UNKNOWN",
                "new_state": "IN_STOCK",
            }))
            .is_err()
        );
    }

    #[test]
    fn should_serialize_source_currency_explicitly() -> Result<(), Box<dyn std::error::Error>> {
        let change = NotificationWatchlistChange::PriceChange {
            old_price: Some(Price::new(MonetaryAmount::from(1000_u64), Currency::Eur)),
            new_price: Some(Price::new(MonetaryAmount::from(900_u64), Currency::Eur)),
        };
        let persisted = NotificationWatchlistChangeV1::from(&change);

        assert_eq!(
            serde_json::json!({
                "type": "PRICE_CHANGE",
                "old_price": { "currency": "EUR", "amount": 1000 },
                "new_price": { "currency": "EUR", "amount": 900 },
            }),
            serde_json::to_value(persisted)?
        );
        Ok(())
    }
}
