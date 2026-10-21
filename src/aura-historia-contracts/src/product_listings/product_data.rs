use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedule};

use domain_primitives::event_id::EventId;

use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;

use notification_core::{
    notification_id::NotificationId, presentation::NotificationImagePresentation,
};
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{ProductListingAuction, ProductListingPricing};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::ports::{ListingSourceSummary, ProductListingAuctionSummary};
use product_listing_service::use_cases::{
    DisplayProductListingPricing, PersonalizedProductListingDetailsView,
    PersonalizedProductListingSummary, ProductListingDetailsView,
    ProductListingPricingPresentation, ProductListingPricingValuation, ProductListingSummary,
    ProductListingSummaryPriceValuation,
};
use product_listing_service::user_state::{
    ContentVisibilityUserState, NotificationUserState, ProductListingUserStateView,
    SearchFilterUserState, WatchlistUserState,
};
use schemars::JsonSchema;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalizedData<ItemData, UserStateData> {
    pub item: ItemData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_state: Option<UserStateData>,
}

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingDetailsData {
    shipping: Option<crate::shipping::listing::AdvertisedShippingData>,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    product_listing_title_slug_id: Option<ProductListingSlugId>,
    #[schemars(schema_with = "crate::wire::schema::event_id")]
    event_id: EventId,
    source: ListingSourceSummaryData,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    #[schemars(with = "String")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingPresentationData,
    #[serde(with = "crate::wire::listing::availability_option")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing::lifecycle")]
    #[schemars(schema_with = "crate::wire::listing::lifecycle_schema")]
    lifecycle: ListingLifecycle,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(schema_with = "crate::wire::schema::url")]
    view_url: Url,
    images: Vec<ProductListingImageData>,
    content_policy: Option<ContentPolicyData>,
    auction: Option<AuctionSummaryData>,
    lot: Option<ProductListingLotData>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingUserStateData {
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping: Option<crate::shipping::listing::PersonalizedShippingData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    watchlist: Option<WatchlistUserStateData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_visibility: Option<ContentVisibilityUserStateData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notification: Option<NotificationUserStateData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_filter: Option<SearchFilterUserStateData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct WatchlistUserStateData {
    watching: bool,
    notifications: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ContentVisibilityUserStateData {
    show_unassessed_or_sensitive_content: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct NotificationUserStateData {
    #[schemars(schema_with = "crate::wire::schema::notification_ids")]
    unseen_notification_ids: Vec<NotificationId>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SearchFilterUserStateData {
    matched: bool,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_user_search_filter_id")]
    user_search_filter_id: Option<UserSearchFilterId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    user_search_filter_name: Option<UserSearchFilterName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    match_feedback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shipping_match_valuation: Option<crate::shipping::listing::ShippingMatchValuationData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ListingSourceSummaryData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    name: String,
    slug_id: String,
}

impl From<ListingSourceSummary> for ListingSourceSummaryData {
    fn from(source: ListingSourceSummary) -> Self {
        Self {
            listing_source_id: source.listing_source_id,
            name: source.name.as_ref().to_owned(),
            slug_id: source.slug_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingSummaryData {
    shipping: Option<crate::shipping::listing::AdvertisedShippingData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geography: Option<crate::geographic_search::ListingGeographyData>,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    product_listing_title_slug_id: Option<ProductListingSlugId>,
    #[schemars(schema_with = "crate::wire::schema::event_id")]
    event_id: EventId,
    source: ListingSourceSummaryData,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    #[schemars(with = "String")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_auction_id")]
    auction_id: Option<AuctionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_price: Option<ProductListingPriceData>,
    price_valuation: ProductListingSummaryPriceValuationData,
    #[serde(with = "crate::wire::listing::availability_option")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing::lifecycle")]
    #[schemars(schema_with = "crate::wire::listing::lifecycle_schema")]
    lifecycle: ListingLifecycle,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(schema_with = "crate::wire::schema::url")]
    view_url: Url,
    images: Vec<ProductListingImageData>,
    content_policy: Option<ContentPolicyData>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingPresentationData {
    source: ProductListingPricingData,
    display: ProductListingPricingData,
    valuation: ProductListingPricingValuationData,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<ProductListingPriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingPricingValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        #[schemars(schema_with = "crate::wire::schema::datetime")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        #[schemars(schema_with = "crate::wire::schema::datetime")]
        captured_at: OffsetDateTime,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        #[schemars(schema_with = "crate::wire::schema::datetime")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingSummaryPriceValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        #[schemars(schema_with = "crate::wire::schema::datetime")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
        fx_rate_id: FxRateId,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        #[schemars(schema_with = "crate::wire::schema::datetime")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingImageData {
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    url: Option<Url>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
enum ContentPolicyData {
    Allowed,
    RequiresConsent { category: &'static str },
}

impl From<ContentPolicyDecision> for ContentPolicyData {
    fn from(value: ContentPolicyDecision) -> Self {
        match value {
            ContentPolicyDecision::Allowed => Self::Allowed,
            ContentPolicyDecision::RequiresConsent(category) => Self::RequiresConsent {
                category: category.as_str(),
            },
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingAuctionData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_auction_id")]
    auction_id: Option<AuctionId>,
    lot_number: Option<String>,
    #[schemars(range(max = u32::MAX))]
    catalogue_position: Option<u32>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_closes: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    reported_closed_at: Option<OffsetDateTime>,
}

impl ProductListingDetailsData {
    fn from_view(view: ProductListingDetailsView) -> Self {
        Self {
            shipping: view
                .shipping
                .map(crate::shipping::listing::AdvertisedShippingData),
            product_listing_id: view.product_listing_id,
            product_listing_title_slug_id: view.product_listing_title_slug_id,
            event_id: view.event_id,
            source: view.source.into(),
            source_listing_id: view.source_listing_id,
            product_title: view.product_title.map(Into::into),
            product_description: view.product_description.map(Into::into),
            title: view.title.map(Into::into),
            description: view.description.map(Into::into),
            pricing: view.pricing.into(),
            availability: view.availability,
            lifecycle: view.lifecycle,
            url: view.url,
            view_url: view.view_url,
            images: view.images.into_iter().map(Into::into).collect(),
            content_policy: view.content_policy.map(Into::into),
            auction: view.auction.map(Into::into),
            lot: view.lot.map(Into::into),
            created: view.created,
            updated: view.updated,
        }
    }
}

impl From<ProductListingDetailsView> for ProductListingDetailsData {
    fn from(view: ProductListingDetailsView) -> Self {
        Self::from_view(view)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingLotData {
    lot_number: Option<String>,
    #[schemars(range(max = u32::MAX))]
    catalogue_position: Option<u32>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_closes: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    reported_closed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AuctionSummaryData {
    #[schemars(schema_with = "crate::wire::schema::auction_id")]
    auction_id: AuctionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reported_status: Option<&'static str>,
    schedule: AuctionScheduleData,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AuctionScheduleData {
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    live_starts: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    lots_begin_closing: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_end: Option<OffsetDateTime>,
}

impl From<ProductListingAuctionSummary> for AuctionSummaryData {
    fn from(value: ProductListingAuctionSummary) -> Self {
        Self {
            auction_id: value.auction_id,
            name: value.name.map(|name| LocalizedTextData {
                text: name.payload.as_ref().to_owned(),
                language: name.localization,
            }),
            format: value.format.map(AuctionFormat::as_str),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            schedule: value.schedule.into(),
        }
    }
}

impl From<AuctionSchedule> for AuctionScheduleData {
    fn from(value: AuctionSchedule) -> Self {
        Self {
            bidding_opens: value.bidding_opens(),
            live_starts: value.live_starts(),
            lots_begin_closing: value.lots_begin_closing(),
            scheduled_end: value.scheduled_end(),
        }
    }
}

impl From<product_listing_service::ports::ProductListingLot> for ProductListingLotData {
    fn from(lot: product_listing_service::ports::ProductListingLot) -> Self {
        Self {
            lot_number: lot.lot_number.map(Into::into),
            catalogue_position: lot.catalogue_position.map(|position| position.value()),
            bidding_opens: lot.bidding_opens,
            scheduled_closes: lot.scheduled_closes,
            reported_closed_at: lot.reported_closed_at,
        }
    }
}

impl From<ProductListingAuction> for ProductListingAuctionData {
    fn from(auction: ProductListingAuction) -> Self {
        Self {
            auction_id: auction.auction_id(),
            lot_number: auction.lot_number().map(ToString::to_string),
            catalogue_position: auction
                .catalogue_position()
                .map(|position| position.value()),
            bidding_opens: auction.bidding_opens(),
            scheduled_closes: auction.scheduled_closes(),
            reported_closed_at: auction.reported_closed_at(),
        }
    }
}

impl From<ProductListingPricingPresentation> for ProductListingPricingPresentationData {
    fn from(pricing: ProductListingPricingPresentation) -> Self {
        Self {
            source: pricing.source.into(),
            display: pricing.display.into(),
            valuation: pricing.valuation.into(),
        }
    }
}

impl From<ProductListingPricing> for ProductListingPricingData {
    fn from(pricing: ProductListingPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<DisplayProductListingPricing> for ProductListingPricingData {
    fn from(pricing: DisplayProductListingPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ProductListingSummaryPriceValuation> for ProductListingSummaryPriceValuationData {
    fn from(valuation: ProductListingSummaryPriceValuation) -> Self {
        match valuation {
            ProductListingSummaryPriceValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductListingSummaryPriceValuation::SaleObservation {
                fx_rate_id,
                observed_at,
            } => Self::SaleObservation {
                fx_rate_id,
                observed_at,
            },
        }
    }
}

impl From<ProductListingPricingValuation> for ProductListingPricingValuationData {
    fn from(valuation: ProductListingPricingValuation) -> Self {
        match valuation {
            ProductListingPricingValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductListingPricingValuation::SaleObservation {
                fx_rate_id,
                captured_at,
                observed_at,
            } => Self::SaleObservation {
                fx_rate_id,
                captured_at,
                observed_at,
            },
        }
    }
}

impl From<ProductListingUserStateView> for ProductListingUserStateData {
    fn from(state: ProductListingUserStateView) -> Self {
        Self {
            shipping: state
                .shipping
                .map(crate::shipping::listing::PersonalizedShippingData),
            watchlist: state.watchlist.map(Into::into),
            content_visibility: state.content_visibility.map(Into::into),
            notification: state.notification.map(Into::into),
            search_filter: state.search_filter.map(Into::into),
        }
    }
}

impl From<WatchlistUserState> for WatchlistUserStateData {
    fn from(state: WatchlistUserState) -> Self {
        Self {
            watching: state.watching,
            notifications: state.notifications,
        }
    }
}

impl From<ContentVisibilityUserState> for ContentVisibilityUserStateData {
    fn from(state: ContentVisibilityUserState) -> Self {
        Self {
            show_unassessed_or_sensitive_content: state.show_unassessed_or_sensitive_content,
        }
    }
}

impl From<NotificationUserState> for NotificationUserStateData {
    fn from(state: NotificationUserState) -> Self {
        Self {
            unseen_notification_ids: state.unseen_notification_ids,
        }
    }
}

impl From<SearchFilterUserState> for SearchFilterUserStateData {
    fn from(state: SearchFilterUserState) -> Self {
        Self {
            matched: state.matched,
            hidden: state.hidden,
            user_search_filter_id: state.user_search_filter_id,
            user_search_filter_name: state.user_search_filter_name,
            match_feedback: state.match_feedback,
            shipping_match_valuation: state.shipping_match_valuation.map(Into::into),
        }
    }
}

impl ProductListingSummaryData {
    fn from_view(summary: ProductListingSummary) -> Self {
        Self {
            shipping: summary
                .shipping
                .map(crate::shipping::listing::AdvertisedShippingData),
            product_listing_id: summary.product_listing_id,
            product_listing_title_slug_id: summary.product_listing_title_slug_id,
            event_id: summary.event_id,
            source: summary.source.into(),
            source_listing_id: summary.source_listing_id,
            auction_id: summary.auction_id,
            title: summary.title.map(Into::into),
            display_price: summary.display_price.map(Into::into),
            price_valuation: summary.price_valuation.into(),
            availability: summary.availability,
            lifecycle: summary.lifecycle,
            url: summary.url,
            view_url: summary.view_url,
            images: summary.images.into_iter().map(Into::into).collect(),
            content_policy: summary.content_policy.map(Into::into),
            geography: summary.geography.map(Into::into),
            updated: summary.updated,
        }
    }
}

impl From<ProductListingSummary> for ProductListingSummaryData {
    fn from(summary: ProductListingSummary) -> Self {
        Self::from_view(summary)
    }
}

impl ProductListingImageData {
    pub fn from_presented(image: NotificationImagePresentation) -> Self {
        Self { url: image.url }
    }
}

impl From<product_listing_service::use_cases::ProductListingImageView> for ProductListingImageData {
    fn from(image: product_listing_service::use_cases::ProductListingImageView) -> Self {
        Self { url: image.url }
    }
}

pub type PersonalizedProductListingDetailsData =
    PersonalizedData<ProductListingDetailsData, ProductListingUserStateData>;
pub type PersonalizedProductListingSummaryData =
    PersonalizedData<ProductListingSummaryData, ProductListingUserStateData>;

pub fn personalized_product_details_data(
    personalized: PersonalizedProductListingDetailsView,
) -> PersonalizedProductListingDetailsData {
    PersonalizedData {
        item: ProductListingDetailsData::from_view(personalized.item),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub fn personalized_product_summary_data(
    personalized: PersonalizedProductListingSummary,
) -> PersonalizedProductListingSummaryData {
    PersonalizedData {
        item: ProductListingSummaryData::from_view(personalized.item),
        user_state: personalized.user_state.map(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::AuctionName;
    use localization::Localized;
    use time::macros::datetime;

    #[test]
    fn should_serialize_only_safe_full_detail_auction_parent_fields_with_exact_timestamps() {
        let auction_id = AuctionId::new();
        let summary = ProductListingAuctionSummary {
            auction_id,
            name: Some(Localized::new(
                localization::Language::En,
                AuctionName::try_from("Autumn Decorative Arts")
                    .unwrap_or_else(|error| panic!("valid test auction name: {error}")),
            )),
            format: Some(AuctionFormat::Timed),
            reported_status: Some(AuctionReportedStatus::Scheduled),
            schedule: AuctionSchedule::new(
                None,
                Some(datetime!(2026-10-18 16:00 UTC)),
                None,
                Some(datetime!(2026-10-19 16:00 UTC)),
            )
            .unwrap_or_else(|error| panic!("valid test auction schedule: {error}")),
        };

        let value = serde_json::to_value(AuctionSummaryData::from(summary))
            .unwrap_or_else(|error| panic!("serialize auction summary: {error}"));

        assert_eq!(
            Some(&serde_json::Value::String(auction_id.to_string())),
            value.get("auctionId")
        );
        assert_eq!(
            Some(&serde_json::Value::String("TIMED".to_owned())),
            value.get("format")
        );
        assert_eq!(
            Some(&serde_json::Value::String("SCHEDULED".to_owned())),
            value.get("reportedStatus")
        );
        assert_eq!(
            Some(&serde_json::Value::String(
                "2026-10-18T16:00:00Z".to_owned()
            )),
            value.pointer("/schedule/liveStarts")
        );
        assert_eq!(
            Some(&serde_json::Value::String(
                "2026-10-19T16:00:00Z".to_owned()
            )),
            value.pointer("/schedule/scheduledEnd")
        );
        assert!(value.get("sourceAuctionId").is_none());
        assert!(value.get("version").is_none());
    }

    #[test]
    fn should_omit_undisclosed_user_state_sections() -> Result<(), serde_json::Error> {
        let data = ProductListingUserStateData::from(ProductListingUserStateView {
            notification: Some(NotificationUserState {
                unseen_notification_ids: Vec::new(),
            }),
            ..Default::default()
        });

        assert_eq!(
            serde_json::json!({"notification": {"unseenNotificationIds": []}}),
            serde_json::to_value(data)?
        );
        Ok(())
    }

    #[test]
    fn should_serialize_a_presented_redacted_image() {
        let data = ProductListingImageData::from(
            product_listing_service::use_cases::ProductListingImageView { url: None },
        );
        assert_eq!(data.url, None);
    }
}
