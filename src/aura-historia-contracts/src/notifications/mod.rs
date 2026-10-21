//! Shared notification inbox wire contracts and presentation.
mod query;
mod wire;
pub use query::{
    InvalidNotificationCursor, ListNotificationsQuery, NotificationsPageData,
    parse_notification_cursor,
};

use crate::product_listings::product_data::ProductListingImageData;
use crate::values::{LocalizedTextData, ProductListingPriceData};
use listing_source_core::listing_source_id::ListingSourceId;
use localization::{Language, Localized};

use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange,
        LocalizedProductListingNotificationSnapshot, PartnershipApplicationDecision,
    },
    notification_id::NotificationId,
    notification_kind::NotificationKind,
    presentation::present_image,
};
use notification_service::presentation::NotificationPresentationPreferences;
use notification_service::use_cases::queries::list_notifications::ListedNotification;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice, title::Title,
};
use schemars::JsonSchema;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationSeenData {
    pub seen: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationsSeenData {
    #[schemars(schema_with = "crate::wire::schema::notification_ids")]
    pub notification_ids: Vec<String>,
    pub seen: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationData {
    #[schemars(schema_with = "crate::wire::schema::notification_id")]
    notification_id: NotificationId,
    seen: bool,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    updated: OffsetDateTime,
    #[serde(with = "crate::notifications::wire::kind")]
    #[schemars(schema_with = "wire::kind::schema")]
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
            notification_id: value.notification_id,
            seen: value.seen,
            created: value.created,
            updated: value.updated,
            kind: value.kind,
            payload: (value.content, presentation_preferences).into(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
enum NotificationContentData {
    Watchlist(WatchlistNotificationPayloadData),
    SearchFilter(SearchFilterNotificationPayloadData),
    PartnershipApplication(PartnershipApplicationNotificationPayloadData),
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WatchlistNotificationPayloadData {
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(schema_with = "crate::wire::schema::url")]
    view_url: Url,
    change: WatchlistNotificationChangeData,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum WatchlistNotificationChangeData {
    PriceChange {
        old_price: Option<ProductListingPriceData>,
        new_price: Option<ProductListingPriceData>,
    },
    AvailabilityChange {
        #[serde(with = "crate::wire::listing::availability_option")]
        #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
        old_availability: Option<ListingAvailability>,
        #[serde(with = "crate::wire::listing::availability_option")]
        #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
        new_availability: Option<ListingAvailability>,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchFilterNotificationPayloadData {
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[schemars(schema_with = "crate::wire::schema::user_search_filter_id")]
    user_search_filter_id: UserSearchFilterId,
    user_search_filter_name: String,
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(schema_with = "crate::wire::schema::url")]
    view_url: Url,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PartnershipApplicationNotificationPayloadData {
    #[schemars(schema_with = "wire::partnership_application_id_schema")]
    partnership_application_id: PartnershipApplicationId,
    #[serde(with = "crate::notifications::wire::decision")]
    #[schemars(schema_with = "wire::decision::schema")]
    decision: PartnershipApplicationDecision,
    party_name: String,
    listing_source_name: String,
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
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
                product_listing_id,
                snapshot,
                change,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::Watchlist(WatchlistNotificationPayloadData {
                    product_listing_id,
                    listing_source_id: snapshot.listing_source_id,
                    source_listing_id: snapshot.source_listing_id,
                    listing_source_slug_id: snapshot.listing_source_slug_id,
                    product_listing_title_slug_id: snapshot.product_listing_title_slug_id,
                    listing_source_name: snapshot.listing_source_name,
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                    change: change.into(),
                })
            }
            LocalizedNotificationContent::SearchFilter {
                product_listing_id,
                snapshot,
                user_search_filter_id,
                user_search_filter_name,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::SearchFilter(SearchFilterNotificationPayloadData {
                    product_listing_id,
                    user_search_filter_id,
                    user_search_filter_name: user_search_filter_name.to_string(),
                    listing_source_id: snapshot.listing_source_id,
                    source_listing_id: snapshot.source_listing_id.to_string(),
                    listing_source_slug_id: snapshot.listing_source_slug_id.to_string(),
                    product_listing_title_slug_id: snapshot
                        .product_listing_title_slug_id
                        .to_string(),
                    listing_source_name: snapshot.listing_source_name.to_string(),
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                })
            }

            LocalizedNotificationContent::PartnershipApplication {
                partnership_application_id,
                snapshot,
                decision,
            } => Self::PartnershipApplication(PartnershipApplicationNotificationPayloadData {
                partnership_application_id,
                decision,
                party_name: snapshot.party_name.to_string(),
                listing_source_name: snapshot.listing_source_name.to_string(),
                image: snapshot.image,
            }),
        }
    }
}

fn notification_product_snapshot(
    snapshot: LocalizedProductListingNotificationSnapshot,
    presentation_preferences: NotificationPresentationPreferences,
) -> NotificationProductListingSnapshotData {
    NotificationProductListingSnapshotData {
        listing_source_id: snapshot.listing_source_id,
        source_listing_id: snapshot.source_listing_id.to_string(),
        listing_source_slug_id: snapshot.listing_source_slug_id.to_string(),
        product_listing_title_slug_id: snapshot.product_listing_title_slug_id.to_string(),
        listing_source_name: snapshot.listing_source_name.to_string(),
        title: snapshot.title.map(localized_text_data),
        image: present_image(
            snapshot.image,
            snapshot.content_policy,
            presentation_preferences.show_unassessed_or_sensitive_content,
        )
        .map(ProductListingImageData::from_presented),
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

fn product_listing_price_data(value: ProductListingPrice) -> ProductListingPriceData {
    value.into()
}

struct NotificationProductListingSnapshotData {
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
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
                old_price: old_price.map(product_listing_price_data),
                new_price: new_price.map(product_listing_price_data),
            },
            LocalizedNotificationWatchlistChange::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability,
                new_availability,
            },
        }
    }
}
