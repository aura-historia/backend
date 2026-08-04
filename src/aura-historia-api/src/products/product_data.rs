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
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
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
