use common::{
    currency::domain::Currency,
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    notification_id::NotificationId,
    partner_shop_application_id::PartnerShopApplicationId,
    price::domain::MonetaryAmount,
    product_id::ProductId,
    product_slug_id::ProductSlugId,
    product_state::domain::ProductState,
    shop_id::ShopId,
    shop_name::ShopName,
    shop_slug_id::ShopSlugId,
    shops_product_id::ShopsProductId,
    user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use notification_core::{
    notification::{
        Notification, NotificationContent, NotificationWatchlistChange, PartnerApplicationDecision,
        PartnerApplicationNotificationSnapshot, ProductNotificationSnapshot,
        RehydratedNotificationState,
    },
    notification_kind::NotificationKind,
};
use product_core::{product_image::ProductImage, title::Title};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use url::Url;

pub(crate) const PAYLOAD_VERSION: i16 = 1;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct NotificationRow {
    pub(crate) notification_id: uuid::Uuid,
    pub(crate) user_id: uuid::Uuid,
    pub(crate) kind: String,
    pub(crate) origin_event_id: Option<uuid::Uuid>,
    pub(crate) product_id: Option<uuid::Uuid>,
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
        snapshot: ProductNotificationSnapshotV1,
        change: NotificationWatchlistChangeV1,
    },
    SearchFilter {
        snapshot: ProductNotificationSnapshotV1,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductNotificationSnapshotV1 {
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    shop_slug_id: ShopSlugId,
    product_slug_id: ProductSlugId,
    shop_name: ShopName,
    title: Option<Vec<LocalizedTitleV1>>,
    image: Option<ProductImage>,
    url: Url,
    view_url: Url,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum NotificationWatchlistChangeV1 {
    PriceChange {
        old_price: HashMap<Currency, MonetaryAmount>,
        new_price: HashMap<Currency, MonetaryAmount>,
    },
    StateChange {
        old_state: ProductState,
        new_state: ProductState,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartnerApplicationNotificationSnapshotV1 {
    shop_name: ShopName,
    image: Option<Url>,
}

impl From<&ProductNotificationSnapshot> for ProductNotificationSnapshotV1 {
    fn from(snapshot: &ProductNotificationSnapshot) -> Self {
        Self {
            shop_id: snapshot.shop_id,
            shops_product_id: snapshot.shops_product_id.clone(),
            shop_slug_id: snapshot.shop_slug_id.clone(),
            product_slug_id: snapshot.product_slug_id.clone(),
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

impl TryFrom<ProductNotificationSnapshotV1> for ProductNotificationSnapshot {
    type Error = NotificationMappingError;

    fn try_from(snapshot: ProductNotificationSnapshotV1) -> Result<Self, Self::Error> {
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
            shops_product_id: snapshot.shops_product_id,
            shop_slug_id: snapshot.shop_slug_id,
            product_slug_id: snapshot.product_slug_id,
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
                old_price: old_price.clone(),
                new_price: new_price.clone(),
            },
            NotificationWatchlistChange::StateChange {
                old_state,
                new_state,
            } => Self::StateChange {
                old_state: *old_state,
                new_state: *new_state,
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
                old_price,
                new_price,
            },
            NotificationWatchlistChangeV1::StateChange {
                old_state,
                new_state,
            } => Self::StateChange {
                old_state,
                new_state,
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
    pub(crate) product_id: Option<uuid::Uuid>,
    pub(crate) user_search_filter_id: Option<uuid::Uuid>,
    pub(crate) partner_shop_application_id: Option<uuid::Uuid>,
    pub(crate) payload: serde_json::Value,
}

impl TryFrom<&Notification> for NotificationWriteValues {
    type Error = NotificationMappingError;

    fn try_from(notification: &Notification) -> Result<Self, Self::Error> {
        let (
            origin_event_id,
            product_id,
            user_search_filter_id,
            partner_shop_application_id,
            payload,
        ) = match notification.content() {
            NotificationContent::Watchlist {
                origin_event_id,
                product_id,
                snapshot,
                change,
            } => (
                Some(uuid::Uuid::from(*origin_event_id)),
                Some(uuid::Uuid::from(*product_id)),
                None,
                None,
                NotificationPayloadV1::Watchlist {
                    snapshot: snapshot.into(),
                    change: change.into(),
                },
            ),
            NotificationContent::SearchFilter {
                origin_event_id,
                product_id,
                user_search_filter_id,
                snapshot,
                user_search_filter_name,
            } => (
                Some(uuid::Uuid::from(*origin_event_id)),
                Some(uuid::Uuid::from(*product_id)),
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
            kind: notification_kind_db_value(notification.kind()),
            origin_event_id,
            product_id,
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
            row.product_id,
            row.user_search_filter_id,
            row.partner_shop_application_id,
        ) {
            (
                NotificationKind::WatchlistPriceChanged | NotificationKind::WatchlistStateChanged,
                NotificationPayloadV1::Watchlist { snapshot, change },
                Some(origin_event_id),
                Some(product_id),
                None,
                None,
            ) => {
                let change: NotificationWatchlistChange = change.into();
                if change_kind(&change) != kind {
                    return Err(NotificationMappingError::KindPayloadMismatch);
                }
                NotificationContent::Watchlist {
                    origin_event_id: EventId::from(origin_event_id),
                    product_id: ProductId::from(product_id),
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
                Some(product_id),
                Some(user_search_filter_id),
                None,
            ) => NotificationContent::SearchFilter {
                origin_event_id: EventId::from(origin_event_id),
                product_id: ProductId::from(product_id),
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

fn notification_kind_db_value(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::WatchlistPriceChanged => "WATCHLIST_PRICE_CHANGED",
        NotificationKind::WatchlistStateChanged => "WATCHLIST_STATE_CHANGED",
        NotificationKind::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        NotificationKind::PartnerApplicationApproved => "PARTNER_APPLICATION_APPROVED",
        NotificationKind::PartnerApplicationRejected => "PARTNER_APPLICATION_REJECTED",
    }
}

fn parse_language(
    value: &str,
) -> Result<common::language::domain::Language, NotificationMappingError> {
    use common::language::domain::Language;
    match value {
        "de" => Ok(Language::De),
        "en" => Ok(Language::En),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        "it" => Ok(Language::It),
        "zh" => Ok(Language::Zh),
        "pt" => Ok(Language::Pt),
        "pl" => Ok(Language::Pl),
        "tr" => Ok(Language::Tr),
        "nl" => Ok(Language::Nl),
        "cs" => Ok(Language::Cs),
        "ja" => Ok(Language::Ja),
        "ru" => Ok(Language::Ru),
        "ar" => Ok(Language::Ar),
        _ => Err(NotificationMappingError::UnknownLanguage(value.to_owned())),
    }
}

fn parse_kind(value: &str) -> Result<NotificationKind, NotificationMappingError> {
    match value {
        "WATCHLIST_PRICE_CHANGED" => Ok(NotificationKind::WatchlistPriceChanged),
        "WATCHLIST_STATE_CHANGED" => Ok(NotificationKind::WatchlistStateChanged),
        "SEARCH_FILTER_MATCH" => Ok(NotificationKind::SearchFilterMatch),
        "PARTNER_APPLICATION_APPROVED" => Ok(NotificationKind::PartnerApplicationApproved),
        "PARTNER_APPLICATION_REJECTED" => Ok(NotificationKind::PartnerApplicationRejected),
        _ => Err(NotificationMappingError::UnknownKind(value.to_owned())),
    }
}

fn change_kind(change: &NotificationWatchlistChange) -> NotificationKind {
    match change {
        NotificationWatchlistChange::PriceChange { .. } => NotificationKind::WatchlistPriceChanged,
        NotificationWatchlistChange::StateChange { .. } => NotificationKind::WatchlistStateChanged,
    }
}

pub(crate) fn mapping_error(error: NotificationMappingError) -> BoxError {
    box_error(error)
}
