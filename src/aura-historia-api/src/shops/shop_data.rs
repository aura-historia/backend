use application::operation_context::Principal;
use axum::Json;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use localization::Language;
use money::Currency;
use serde::Serialize;
use serde_email::Email;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use shop_service::use_cases::queries::get_shop::ShopDetailsView;
use shop_service::use_cases::queries::search_shops::ShopSummary;
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShopData {
    shop_id: ShopId,
    shop_slug_id: shop_core::shop_slug_id::ShopSlugId,
    name: shop_core::shop_name::ShopName,
    #[serde(with = "crate::wire::shop_type")]
    shop_type: ShopType,
    domains: HashSet<shop_core::domain::Domain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shopify_domain: Option<shop_core::domain::Domain>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::currency::option"
    )]
    shopify_currency: Option<Currency>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::language::option"
    )]
    shopify_language: Option<Language>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::currency::option"
    )]
    woocommerce_currency: Option<Currency>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::language::option"
    )]
    woocommerce_language: Option<Language>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view_url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geo_address: Option<GeoAddressData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<Email>,
    #[serde(with = "crate::wire::shop_partner_status")]
    partner_status: ShopPartnerStatus,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShopSummaryData {
    shop_id: ShopId,
    shop_slug_id: shop_core::shop_slug_id::ShopSlugId,
    name: shop_core::shop_name::ShopName,
    #[serde(with = "crate::wire::shop_type")]
    shop_type: ShopType,
    domains: Vec<shop_core::domain::Domain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<Url>,
    #[serde(with = "crate::wire::shop_partner_status")]
    partner_status: ShopPartnerStatus,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl From<ShopSummary> for ShopSummaryData {
    fn from(summary: ShopSummary) -> Self {
        Self {
            shop_id: summary.shop_id,
            shop_slug_id: summary.shop_slug_id,
            name: summary.name,
            shop_type: summary.shop_type,
            domains: summary.domains,
            image: summary.image,
            partner_status: summary.partner_status,
            created: summary.created,
            updated: summary.updated,
        }
    }
}

impl From<ShopDetailsView> for ShopData {
    fn from(view: ShopDetailsView) -> Self {
        Self {
            shop_id: view.shop_id,
            shop_slug_id: view.shop_slug_id,
            name: view.name,
            shop_type: view.shop_type,
            domains: view.domains,
            shopify_domain: view.shopify_domain,
            shopify_currency: view.shopify_currency,
            shopify_language: view.shopify_language,
            woocommerce_currency: view.woocommerce_currency,
            woocommerce_language: view.woocommerce_language,
            url: view.url,
            view_url: view.view_url,
            image: view.image,
            structured_address: view.structured_address.map(Into::into),
            geo_address: view.geo_address.map(Into::into),
            phone: view.phone,
            email: view.email,
            partner_status: view.partner_status,
            created: view.created,
            updated: view.updated,
        }
    }
}

pub(crate) fn shop_response(
    view: ShopDetailsView,
    cache_control: Option<&'static str>,
) -> Response {
    let updated = view.updated;
    let mut response = Json(ShopData::from(view)).into_response();
    let headers = response.headers_mut();
    if let Some(cache_control) = cache_control {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(cache_control),
        );
    }
    if let Ok(value) = HeaderValue::from_str(&httpdate::fmt_http_date(updated.into())) {
        headers.insert(header::LAST_MODIFIED, value);
    }
    response
}

pub(crate) fn cache_control(principal: &Principal) -> &'static str {
    match principal {
        Principal::Anonymous => "public, max-age=600, s-maxage=3600",
        Principal::User(_)
        | Principal::DelegatedUser { .. }
        | Principal::Service(_)
        | Principal::System => "no-store",
    }
}
