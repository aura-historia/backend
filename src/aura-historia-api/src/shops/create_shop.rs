use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::shops::shop_data::shop_response;
use crate::state::ShopsState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use geo::data::address_data::StructuredAddressData;
use localization::Language;
use money::Currency;
use serde::Deserialize;
use serde_email::Email;
use shop_core::domain::Domain;
use shop_core::shop_name::ShopName;
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_service::use_cases::commands::create_shop::CreateShopCommand;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateShopData {
    name: ShopName,
    #[serde(with = "crate::wire::shop_type")]
    shop_type: ShopType,
    domains: HashSet<Domain>,
    #[serde(default)]
    shopify_domain: Option<Domain>,
    #[serde(default)]
    #[serde(with = "crate::wire::currency::option")]
    shopify_currency: Option<Currency>,
    #[serde(default)]
    #[serde(with = "crate::wire::language::option")]
    shopify_language: Option<Language>,
    #[serde(default)]
    woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    #[serde(default)]
    #[serde(with = "crate::wire::currency::option")]
    woocommerce_currency: Option<Currency>,
    #[serde(default)]
    #[serde(with = "crate::wire::language::option")]
    woocommerce_language: Option<Language>,
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
        Err(response) => return *response,
    };
    let data = match parse_body(&body) {
        Ok(data) => data,
        Err(error) => return error.into_response(),
    };
    let command = CreateShopCommand {
        name: data.name,
        shop_type: data.shop_type,
        domains: data.domains,
        shopify_domain: data.shopify_domain,
        shopify_currency: data.shopify_currency,
        shopify_language: data.shopify_language,
        woocommerce_webhook_secret: data.woocommerce_webhook_secret,
        woocommerce_currency: data.woocommerce_currency,
        woocommerce_language: data.woocommerce_language,
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
