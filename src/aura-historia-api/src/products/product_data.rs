use crate::values::{LocalizedTextData, PriceData};
use axum::Json;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::operation_context::Principal;
use common::personalized::api::PersonalizedData;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shops_product_id::ShopsProductId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use product_core::product::ProductPricing;
use product_core::prohibited_content::ProhibitedContent;
use product_core::user_state::{
    NotificationUserState, ProductUserState, ProhibitedContentUserState, SearchFilterUserState,
    WatchlistUserState,
};
use product_service::use_cases::{
    DisplayProductPricing, PersonalizedProductDetailsView, PersonalizedProductSummary,
    ProductDetailsView, ProductPricingPresentation, ProductPricingValuation, ProductSummary,
    ProductSummaryPriceValuation,
};
use serde::Serialize;
use shop_core::shop_id::ShopId;
use shop_core::shop_slug_id::ShopSlugId;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductDetailsData {
    product_id: ProductId,
    product_slug_id: ProductSlugId,
    event_id: EventId,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
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
    pricing: ProductPricingPresentationData,
    state: ProductStateData,
    lifecycle: ProductLifecycleData,
    url: Url,
    view_url: Url,
    images: Vec<ProductImageData>,
    auction: ProductAuctionData,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductUserStateData {
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
    seen: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_event_id: Option<EventId>,
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
pub(crate) struct ProductSummaryData {
    product_id: ProductId,
    product_slug_id: ProductSlugId,
    event_id: EventId,
    shop_id: ShopId,
    seller_id: ShopId,
    shops_product_id: ShopsProductId,
    shop_name: String,
    shop_slug_id: ShopSlugId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_price: Option<PriceData>,
    price_valuation: ProductSummaryPriceValuationData,
    state: ProductStateData,
    lifecycle: ProductLifecycleData,
    url: Url,
    view_url: Url,
    images: Vec<ProductImageData>,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductPricingPresentationData {
    source: ProductPricingData,
    display: ProductPricingData,
    valuation: ProductPricingValuationData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductPricingValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    Sale {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
        #[serde(rename = "soldAt", with = "time::serde::rfc3339")]
        sold_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductSummaryPriceValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    Sale {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "soldAt", with = "time::serde::rfc3339")]
        sold_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductImageData {
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    prohibited_content: ProductProhibitedContentData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductProhibitedContentData {
    Unknown,
    None,
    NaziGermany,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductAuctionData {
    #[serde(with = "time::serde::rfc3339::option")]
    start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    end: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductLifecycleData {
    Active,
    Deleted,
}

impl ProductDetailsData {
    fn from_view(view: ProductDetailsView, prohibited_content_consent: bool) -> Self {
        Self {
            product_id: view.product_id,
            product_slug_id: view.product_slug_id,
            event_id: view.event_id,
            shop_id: view.shop_id,
            seller_id: view.seller_id,
            shops_product_id: view.shops_product_id,
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
            state: view.state.into(),
            lifecycle: view.lifecycle.into(),
            url: view.url,
            view_url: view.view_url,
            images: view
                .images
                .into_iter()
                .map(|image| ProductImageData::from_with_consent(image, prohibited_content_consent))
                .collect(),
            auction: ProductAuctionData {
                start: view.auction.start,
                end: view.auction.end,
            },
            created: view.created,
            updated: view.updated,
        }
    }
}

impl From<ProductDetailsView> for ProductDetailsData {
    fn from(view: ProductDetailsView) -> Self {
        Self::from_view(view, false)
    }
}

impl From<ProductPricingPresentation> for ProductPricingPresentationData {
    fn from(pricing: ProductPricingPresentation) -> Self {
        Self {
            source: pricing.source.into(),
            display: pricing.display.into(),
            valuation: pricing.valuation.into(),
        }
    }
}

impl From<ProductPricing> for ProductPricingData {
    fn from(pricing: ProductPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<DisplayProductPricing> for ProductPricingData {
    fn from(pricing: DisplayProductPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ProductSummaryPriceValuation> for ProductSummaryPriceValuationData {
    fn from(valuation: ProductSummaryPriceValuation) -> Self {
        match valuation {
            ProductSummaryPriceValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductSummaryPriceValuation::Sale {
                fx_rate_id,
                sold_at,
            } => Self::Sale {
                fx_rate_id,
                sold_at,
            },
        }
    }
}

impl From<ProductPricingValuation> for ProductPricingValuationData {
    fn from(valuation: ProductPricingValuation) -> Self {
        match valuation {
            ProductPricingValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductPricingValuation::Sale {
                fx_rate_id,
                captured_at,
                sold_at,
            } => Self::Sale {
                fx_rate_id,
                captured_at,
                sold_at,
            },
        }
    }
}

impl From<ProductUserState> for ProductUserStateData {
    fn from(state: ProductUserState) -> Self {
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
            seen: state.seen,
            origin_event_id: state.origin_event_id,
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

impl ProductSummaryData {
    fn from_view(summary: ProductSummary, prohibited_content_consent: bool) -> Self {
        Self {
            product_id: summary.product_id,
            product_slug_id: summary.product_slug_id,
            event_id: summary.event_id,
            shop_id: summary.shop_id,
            seller_id: summary.seller_id,
            shops_product_id: summary.shops_product_id,
            shop_name: summary.shop_name.into(),
            shop_slug_id: summary.shop_slug_id,
            title: summary.title.map(Into::into),
            display_price: summary.display_price.map(Into::into),
            price_valuation: summary.price_valuation.into(),
            state: summary.state.into(),
            lifecycle: summary.lifecycle.into(),
            url: summary.url,
            view_url: summary.view_url,
            images: summary
                .images
                .into_iter()
                .map(|image| ProductImageData::from_with_consent(image, prohibited_content_consent))
                .collect(),
            updated: summary.updated,
        }
    }
}

impl From<ProductSummary> for ProductSummaryData {
    fn from(summary: ProductSummary) -> Self {
        Self::from_view(summary, false)
    }
}

impl ProductImageData {
    pub(crate) fn from_with_consent(
        image: product_core::product_image::ProductImage,
        prohibited_content_consent: bool,
    ) -> Self {
        Self {
            url: (image.prohibited_content.is_safe() || prohibited_content_consent)
                .then_some(image.url),
            prohibited_content: image.prohibited_content.into(),
        }
    }
}

impl From<product_core::product_image::ProductImage> for ProductImageData {
    fn from(image: product_core::product_image::ProductImage) -> Self {
        Self::from_with_consent(image, false)
    }
}

impl From<ProhibitedContent> for ProductProhibitedContentData {
    fn from(value: ProhibitedContent) -> Self {
        match value {
            ProhibitedContent::Unknown => Self::Unknown,
            ProhibitedContent::None => Self::None,
            ProhibitedContent::NaziGermany => Self::NaziGermany,
        }
    }
}

pub(crate) type PersonalizedProductDetailsData =
    PersonalizedData<ProductDetailsData, ProductUserStateData>;
pub(crate) type PersonalizedProductSummaryData =
    PersonalizedData<ProductSummaryData, ProductUserStateData>;

pub(crate) fn personalized_product_details_data(
    personalized: PersonalizedProductDetailsView,
) -> PersonalizedProductDetailsData {
    let prohibited_content_consent = personalized
        .user_state
        .as_ref()
        .is_some_and(|state| state.prohibited_content.consent);
    PersonalizedData {
        item: ProductDetailsData::from_view(personalized.item, prohibited_content_consent),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub(crate) fn personalized_product_summary_data(
    personalized: PersonalizedProductSummary,
) -> PersonalizedProductSummaryData {
    let prohibited_content_consent = personalized
        .user_state
        .as_ref()
        .is_some_and(|state| state.prohibited_content.consent);
    PersonalizedData {
        item: ProductSummaryData::from_view(personalized.item, prohibited_content_consent),
        user_state: personalized.user_state.map(Into::into),
    }
}

impl From<ProductState> for ProductStateData {
    fn from(state: ProductState) -> Self {
        match state {
            ProductState::Listed => Self::Listed,
            ProductState::Available => Self::Available,
            ProductState::Reserved => Self::Reserved,
            ProductState::Sold => Self::Sold,
            ProductState::Removed => Self::Removed,
            ProductState::Unknown => Self::Unknown,
        }
    }
}

impl From<ProductLifecycle> for ProductLifecycleData {
    fn from(lifecycle: ProductLifecycle) -> Self {
        match lifecycle {
            ProductLifecycle::Active => Self::Active,
            ProductLifecycle::Deleted => Self::Deleted,
        }
    }
}

pub(crate) fn product_response(
    view: PersonalizedProductDetailsView,
    principal: &Principal,
) -> Response {
    let state = view.item.state;
    let content_language = view
        .item
        .title
        .as_ref()
        .map(|title| title.localization.as_str());
    let mut response = Json(personalized_product_details_data(view)).into_response();
    let cache_control = match principal {
        Principal::Anonymous if matches!(state, ProductState::Sold | ProductState::Removed) => {
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
    use product_core::product_image::ProductImage;
    use serde_json::json;

    #[test]
    fn should_serialize_sale_pricing_valuation() -> Result<(), Box<dyn std::error::Error>> {
        let fx_rate_id = FxRateId::new();
        let data = ProductPricingValuationData::Sale {
            fx_rate_id,
            captured_at: OffsetDateTime::UNIX_EPOCH,
            sold_at: OffsetDateTime::UNIX_EPOCH,
        };

        assert_eq!(
            serde_json::to_value(data)?,
            json!({
                "type": "SALE",
                "fxRateId": fx_rate_id,
                "capturedAt": "1970-01-01T00:00:00Z",
                "soldAt": "1970-01-01T00:00:00Z"
            })
        );
        Ok(())
    }

    #[test]
    fn should_expose_safe_image_url_without_prohibited_content_consent()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = ProductImageData::from_with_consent(image(ProhibitedContent::None)?, false);

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
        let data = ProductImageData::from(image(ProhibitedContent::NaziGermany)?);

        assert_eq!(
            serde_json::to_value(data)?,
            json!({ "prohibitedContent": "NAZI_GERMANY" })
        );
        Ok(())
    }

    #[test]
    fn should_expose_unsafe_image_url_with_prohibited_content_consent()
    -> Result<(), Box<dyn std::error::Error>> {
        let data = ProductImageData::from_with_consent(image(ProhibitedContent::Unknown)?, true);

        assert_eq!(
            serde_json::to_value(data)?,
            json!({
                "url": "https://shop.example/image.jpg",
                "prohibitedContent": "UNKNOWN"
            })
        );
        Ok(())
    }

    fn image(prohibited_content: ProhibitedContent) -> Result<ProductImage, url::ParseError> {
        Ok(ProductImage {
            url: Url::parse("https://shop.example/image.jpg")?,
            prohibited_content,
        })
    }
}
