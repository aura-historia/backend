use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::shops::shop_data::shop_response;
use crate::shops::types::ShopTypeData;
use crate::state::ShopsState;
use crate::values::{CurrencyData, LanguageData};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use geo::data::address_data::StructuredAddressData;
use serde::Deserialize;
use serde_email::Email;
use shop_core::domain::Domain;
use shop_core::shop_name::ShopName;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_service::use_cases::commands::create_shop::CreateShopCommand;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateShopData {
    name: ShopName,
    shop_type: ShopTypeData,
    domains: HashSet<Domain>,
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

pub async fn create_shop(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let command = CreateShopCommand {
        name: data.name,
        shop_type: data.shop_type.into(),
        domains: data.domains,
        shopify_domain: data.shopify_domain,
        shopify_currency: data.shopify_currency.map(Into::into),
        shopify_language: data.shopify_language.map(Into::into),
        woocommerce_webhook_secret: data.woocommerce_webhook_secret,
        woocommerce_currency: data.woocommerce_currency.map(Into::into),
        woocommerce_language: data.woocommerce_language.map(Into::into),
        url: data.url,
        image: data.image,
        structured_address: data.structured_address.map(Into::into),
        phone: data.phone,
        email: data.email,
        affiliate_configuration: None,
    };
    match state.create_shop.execute(&context, command).await {
        Ok(view) => {
            let mut response = shop_response(view, None);
            *response.status_mut() = StatusCode::CREATED;
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<CreateShopData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty"));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
