use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::shops::shop_data::shop_response;
use crate::shops::types::ShopTypeData;
use crate::state::ShopsState;
use crate::values::{CurrencyData, LanguageData};
use application::patch_field::PatchField;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use geo::data::address_data::StructuredAddressData;
use serde::Deserialize;
use serde_email::Email;
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_service::use_cases::commands::update_shop::UpdateShopCommand;

use std::collections::HashSet;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShopData {
    #[serde(default)]
    shop_type: Option<ShopTypeData>,
    #[serde(default)]
    domains: Option<HashSet<Domain>>,
    #[serde(default)]
    shopify_domain: Option<Domain>,
    #[serde(default)]
    shopify_currency: Option<CurrencyData>,
    #[serde(default)]
    shopify_language: Option<LanguageData>,
    #[serde(default)]
    woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    #[serde(default)]
    woocommerce_currency: Option<CurrencyData>,
    #[serde(default)]
    woocommerce_language: Option<LanguageData>,
    #[serde(default)]
    url: Option<Url>,
    #[serde(default)]
    image: Option<Url>,
    #[serde(default)]
    structured_address: Option<StructuredAddressData>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    email: Option<Email>,
}

pub async fn update_shop(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    Path(raw_shop_id): Path<String>,
    body: String,
) -> Response {
    let shop_id = match ShopId::try_from(raw_shop_id.as_str()) {
        Ok(shop_id) => shop_id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("shopId")
                .with_detail("Path parameter 'shopId' must be a UUID.")
                .into_response();
        }
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let command = UpdateShopCommand {
        shop_id,
        shop_type: patch(data.shop_type.map(Into::into)),
        domains: patch(data.domains),
        shopify_domain: patch(data.shopify_domain),
        shopify_currency: patch(data.shopify_currency.map(Into::into)),
        shopify_language: patch(data.shopify_language.map(Into::into)),
        woocommerce_webhook_secret: patch(data.woocommerce_webhook_secret),
        woocommerce_currency: patch(data.woocommerce_currency.map(Into::into)),
        woocommerce_language: patch(data.woocommerce_language.map(Into::into)),
        url: patch(data.url),
        image: patch(data.image),
        structured_address: patch(data.structured_address.map(Into::into)),
        phone: patch(data.phone),
        email: patch(data.email),
        affiliate_configuration: PatchField::Unchanged,
    };
    match state.update_shop.execute(&context, command).await {
        Ok(view) => shop_response(view, None),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn patch<T>(value: Option<T>) -> PatchField<T> {
    value.map(PatchField::Set).unwrap_or(PatchField::Unchanged)
}

fn parse_body(body: &str) -> Result<UpdateShopData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty"));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
