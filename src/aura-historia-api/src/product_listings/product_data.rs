use crate::values::{LocalizedTextData, PriceData};
use application::operation_context::Principal;
use axum::Json;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::event_id::EventId;

use fxrate_core::FxRateId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use notification_core::{
    notification_id::NotificationId, presentation::NotificationImagePresentation,
};
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::ProductListingPricing;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_service::use_cases::{
    DisplayProductListingPricing, PersonalizedProductListingDetailsView,
    PersonalizedProductListingSummary, ProductListingDetailsView,
    ProductListingPricingPresentation, ProductListingPricingValuation, ProductListingSummary,
    ProductListingSummaryPriceValuation,
};
use product_listing_service::user_state::{
    NotificationUserState, ProductListingUserState, ProhibitedContentUserState,
    SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersonalizedData<ItemData, UserStateData> {
    pub(crate) item: ItemData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_state: Option<UserStateData>,
}
use shop_core::shop_id::ShopId;
use shop_core::shop_slug_id::ShopSlugId;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingDetailsData {
    product_listing_id: ProductListingId,
    product_listing_slug_id: ProductListingSlugId,
    event_id: EventId,
    shop_id: ShopId,
    seller_id: ShopId,
    shop_listing_id: ShopListingId,
    shop_name: String,
    seller_name: String,
    shop_slug_id: ShopSlugId,
    seller_slug_id: ShopSlugId,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingPresentationData,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing_lifecycle")]
    lifecycle: ListingLifecycle,
    url: Url,
    view_url: Url,
    images: Vec<ProductListingImageData>,
    auction: ProductListingAuctionData,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingUserStateData {
    watchlist: WatchlistUserStateData,
    prohibited_content: ProhibitedContentUserStateData,
    notification: NotificationUserStateData,
    search_filter: SearchFilterUserStateData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistUserStateData {
    watching: bool,
    notifications: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProhibitedContentUserStateData {
    consent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationUserStateData {
    unseen_notification_ids: Vec<NotificationId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilterUserStateData {
    matched: bool,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_id: Option<UserSearchFilterId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_name: Option<UserSearchFilterName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    match_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    match_feedback: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingSummaryData {
    product_listing_id: ProductListingId,
    product_listing_slug_id: ProductListingSlugId,
    event_id: EventId,
    shop_id: ShopId,
    seller_id: ShopId,
    shop_listing_id: ShopListingId,
    shop_name: String,
    shop_slug_id: ShopSlugId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_price: Option<PriceData>,
    price_valuation: ProductListingSummaryPriceValuationData,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::listing_availability::option"
    )]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing_lifecycle")]
    lifecycle: ListingLifecycle,
    url: Url,
    view_url: Url,
    images: Vec<ProductListingImageData>,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingPresentationData {
    source: ProductListingPricingData,
    display: ProductListingPricingData,
    valuation: ProductListingPricingValuationData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingPricingValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingSummaryPriceValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingImageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(with = "crate::wire::prohibited_content")]
    prohibited_content: ProhibitedContent,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingAuctionData {
    #[serde(with = "time::serde::rfc3339::option")]
    start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    end: Option<OffsetDateTime>,
}

impl ProductListingDetailsData {
    fn from_view(view: ProductListingDetailsView, prohibited_content_consent: bool) -> Self {
        Self {
            product_listing_id: view.product_listing_id,
            product_listing_slug_id: view.product_listing_slug_id,
            event_id: view.event_id,
            shop_id: view.shop_id,
            seller_id: view.seller_id,
            shop_listing_id: view.shop_listing_id,
            shop_name: view.shop_name.into(),
            seller_name: view.seller_name.into(),
            shop_slug_id: view.shop_slug_id,
            seller_slug_id: view.seller_slug_id,
            structured_address: view.address.structured.map(Into::into),
            geo_address: view.address.geo.map(Into::into),
            product_title: view.product_title.map(Into::into),
            product_description: view.product_description.map(Into::into),
            title: view.title.map(Into::into),
            description: view.description.map(Into::into),
            pricing: view.pricing.into(),
            availability: view.availability,
            lifecycle: view.lifecycle,
            url: view.url,
            view_url: view.view_url,
            images: view
                .images
                .into_iter()
                .map(|image| {
                    ProductListingImageData::from_with_consent(image, prohibited_content_consent)
                })
                .collect(),
            auction: ProductListingAuctionData {
                start: view.auction.start,
                end: view.auction.end,
            },
            created: view.created,
            updated: view.updated,
        }
    }
}

impl From<ProductListingDetailsView> for ProductListingDetailsData {
    fn from(view: ProductListingDetailsView) -> Self {
        Self::from_view(view, false)
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

impl From<ProductListingUserState> for ProductListingUserStateData {
    fn from(state: ProductListingUserState) -> Self {
        Self {
            watchlist: state.watchlist.into(),
            prohibited_content: state.prohibited_content.into(),
            notification: state.notification.into(),
            search_filter: state.search_filter.into(),
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

impl From<ProhibitedContentUserState> for ProhibitedContentUserStateData {
    fn from(state: ProhibitedContentUserState) -> Self {
        Self {
            consent: state.consent,
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
            match_reason: state.match_reason.map(|reason| reason.to_string()),
            match_feedback: state.match_feedback,
        }
    }
}

impl ProductListingSummaryData {
    fn from_view(summary: ProductListingSummary, prohibited_content_consent: bool) -> Self {
        Self {
            product_listing_id: summary.product_listing_id,
            product_listing_slug_id: summary.product_listing_slug_id,
            event_id: summary.event_id,
            shop_id: summary.shop_id,
            seller_id: summary.seller_id,
            shop_listing_id: summary.shop_listing_id,
            shop_name: summary.shop_name.into(),
            shop_slug_id: summary.shop_slug_id,
            title: summary.title.map(Into::into),
            display_price: summary.display_price.map(Into::into),
            price_valuation: summary.price_valuation.into(),
            availability: summary.availability,
            lifecycle: summary.lifecycle,
            url: summary.url,
            view_url: summary.view_url,
            images: summary
                .images
                .into_iter()
                .map(|image| {
                    ProductListingImageData::from_with_consent(image, prohibited_content_consent)
                })
                .collect(),
            updated: summary.updated,
        }
    }
}

impl From<ProductListingSummary> for ProductListingSummaryData {
    fn from(summary: ProductListingSummary) -> Self {
        Self::from_view(summary, false)
    }
}

impl ProductListingImageData {
    pub(crate) fn from_presented(image: NotificationImagePresentation) -> Self {
        Self {
            url: image.url,
            prohibited_content: image.prohibited_content,
        }
    }

    pub(crate) fn from_with_consent(
        image: product_listing_core::product_listing_image::ProductListingImage,
        prohibited_content_consent: bool,
    ) -> Self {
        Self {
            url: (image.prohibited_content.is_safe() || prohibited_content_consent)
                .then_some(image.url),
            prohibited_content: image.prohibited_content,
        }
    }
}

impl From<product_listing_core::product_listing_image::ProductListingImage>
    for ProductListingImageData
{
    fn from(image: product_listing_core::product_listing_image::ProductListingImage) -> Self {
        Self::from_with_consent(image, false)
    }
}

pub(crate) type PersonalizedProductListingDetailsData =
    PersonalizedData<ProductListingDetailsData, ProductListingUserStateData>;
pub(crate) type PersonalizedProductListingSummaryData =
    PersonalizedData<ProductListingSummaryData, ProductListingUserStateData>;

pub(crate) fn personalized_product_details_data(
    personalized: PersonalizedProductListingDetailsView,
) -> PersonalizedProductListingDetailsData {
    let prohibited_content_consent = personalized
        .user_state
        .as_ref()
        .is_some_and(|state| state.prohibited_content.consent);
    PersonalizedData {
        item: ProductListingDetailsData::from_view(personalized.item, prohibited_content_consent),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub(crate) fn personalized_product_summary_data(
    personalized: PersonalizedProductListingSummary,
) -> PersonalizedProductListingSummaryData {
    let prohibited_content_consent = personalized
        .user_state
        .as_ref()
        .is_some_and(|state| state.prohibited_content.consent);
    PersonalizedData {
        item: ProductListingSummaryData::from_view(personalized.item, prohibited_content_consent),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub(crate) fn product_response(
    view: PersonalizedProductListingDetailsView,
    principal: &Principal,
) -> Response {
    let lifecycle = view.item.lifecycle;
    let content_language = view
        .item
        .title
        .as_ref()
        .map(|title| title.localization.as_str());
    let mut response = Json(personalized_product_details_data(view)).into_response();
    let cache_control = match principal {
        Principal::Anonymous if matches!(lifecycle, ListingLifecycle::Withdrawn) => {
            "public, max-age=180, s-maxage=86400"
        }
        Principal::Anonymous => "public, max-age=180, s-maxage=900",
        Principal::User(_)
        | Principal::DelegatedUser { .. }
        | Principal::Service(_)
        | Principal::System => "no-store",
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    if let Some(language) = content_language {
        headers.insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(language));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::product_listing_image::ProductListingImage;
    use serde_json::json;

    #[test]
    fn should_serialize_sale_pricing_valuation() -> Result<(), Box<dyn std::error::Error>> {
        let fx_rate_id = FxRateId::new();
        let data = ProductListingPricingValuationData::SaleObservation {
            fx_rate_id,
            captured_at: OffsetDateTime::UNIX_EPOCH,
            observed_at: OffsetDateTime::UNIX_EPOCH,
        };

        assert_eq!(
            serde_json::to_value(data)?,
            json!({
                "type": "SALE_OBSERVATION",
                "fxRateId": fx_rate_id,
                "capturedAt": "1970-01-01T00:00:00Z",
                "observedAt": "1970-01-01T00:00:00Z"
            })
        );
        Ok(())
    }

    #[test]
    fn should_expose_safe_image_url_without_prohibited_content_consent()
    -> Result<(), Box<dyn std::error::Error>> {
        let data =
            ProductListingImageData::from_with_consent(image(ProhibitedContent::None)?, false);

        assert_eq!(
            serde_json::to_value(data)?,
            json!({
                "url": "https://shop.example/image.jpg",
                "prohibitedContent": "NONE"
            })
        );
        Ok(())
    }

    #[test]
    fn should_redact_unsafe_image_url_without_prohibited_content_consent()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = ProductListingImageData::from(image(ProhibitedContent::NaziGermany)?);

        assert_eq!(
            serde_json::to_value(data)?,
            json!({ "prohibitedContent": "NAZI_GERMANY" })
        );
        Ok(())
    }

    #[test]
    fn should_expose_unsafe_image_url_with_prohibited_content_consent()
    -> Result<(), Box<dyn std::error::Error>> {
        let data =
            ProductListingImageData::from_with_consent(image(ProhibitedContent::Unknown)?, true);

        assert_eq!(
            serde_json::to_value(data)?,
            json!({
                "url": "https://shop.example/image.jpg",
                "prohibitedContent": "UNKNOWN"
            })
        );
        Ok(())
    }

    fn image(
        prohibited_content: ProhibitedContent,
    ) -> Result<ProductListingImage, url::ParseError> {
        Ok(ProductListingImage {
            url: Url::parse("https://shop.example/image.jpg")?,
            prohibited_content,
        })
    }
}
