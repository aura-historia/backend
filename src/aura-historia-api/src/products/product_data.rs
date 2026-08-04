use axum::Json;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use common::currency::data::CurrencyData;
use common::event_id::EventId;
use common::language::data::LocalizedTextData;
use common::operation_context::Principal;
use common::price::data::PriceData;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use product_core::user_state::{
    NotificationUserState, ProductUserState, ProhibitedContentUserState, SearchFilterUserState,
    WatchlistUserState,
};
use product_service::use_cases::{ProductDetailsView, ProductSummary};
use serde::Serialize;
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
    pricing: ProductPricingData,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    currency: Option<CurrencyData>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    user_state: Option<ProductUserStateData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductUserStateData {
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
    price: Option<PriceData>,
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
struct ProductPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fx_rate_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProductImageData {
    url: Url,
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

impl From<ProductDetailsView> for ProductDetailsData {
    fn from(view: ProductDetailsView) -> Self {
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
            pricing: ProductPricingData {
                price: view.pricing.price.map(Into::into),
                price_estimate_min: view.pricing.price_estimate_min.map(Into::into),
                price_estimate_max: view.pricing.price_estimate_max.map(Into::into),
                fx_rate_id: view
                    .pricing
                    .fx_rate_id
                    .map(|fx_rate_id| fx_rate_id.to_string()),
            },
            price: view.price.map(Into::into),
            price_estimate_min: view.price_estimate_min.map(Into::into),
            price_estimate_max: view.price_estimate_max.map(Into::into),
            currency: view.currency.map(Into::into),
            state: view.state.into(),
            lifecycle: view.lifecycle.into(),
            url: view.url,
            view_url: view.view_url,
            images: view
                .images
                .into_iter()
                .map(|image| ProductImageData { url: image.url })
                .collect(),
            auction: ProductAuctionData {
                start: view.auction.start,
                end: view.auction.end,
            },
            created: view.created,
            updated: view.updated,
            user_state: view.user_state.map(Into::into),
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

impl From<ProductSummary> for ProductSummaryData {
    fn from(summary: ProductSummary) -> Self {
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
            price: summary.price.map(Into::into),
            state: summary.state.into(),
            lifecycle: summary.lifecycle.into(),
            url: summary.url,
            view_url: summary.view_url,
            images: summary
                .images
                .into_iter()
                .map(|image| ProductImageData { url: image.url })
                .collect(),
            updated: summary.updated,
        }
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

pub(crate) fn product_response(view: ProductDetailsView, principal: &Principal) -> Response {
    let event_id = view.event_id;
    let updated = view.updated;
    let state = view.state;
    let content_language = view.title.as_ref().map(|title| title.localization.as_str());
    let mut response = Json(ProductDetailsData::from(view)).into_response();
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
    if let Ok(value) = HeaderValue::from_str(&event_id.to_string()) {
        headers.insert(header::ETAG, value);
    }
    if let Ok(value) = HeaderValue::from_str(&httpdate::fmt_http_date(updated.into())) {
        headers.insert(header::LAST_MODIFIED, value);
    }
    response
}
