use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::shops::shop_data::shop_response;
use crate::state::ShopsState;
use application::patch_field::PatchField;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use geo::data::address_data::StructuredAddressData;
use localization::Language;
use money::Currency;
use serde::Deserialize;
use serde_email::Email;
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use shop_core::shop_type::ShopType;
use shop_core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop_service::use_cases::commands::update_shop::UpdateShopCommand;

use std::collections::HashSet;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateShopData {
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::shop_type::patch::deserialize")]
    shop_type: PatchValue<ShopType>,
    #[serde(default)]
    domains: PatchValue<HashSet<Domain>>,
    #[serde(default)]
    shopify_domain: PatchValue<Domain>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::currency::patch::deserialize")]
    shopify_currency: PatchValue<Currency>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::language::patch::deserialize")]
    shopify_language: PatchValue<Language>,
    #[serde(default)]
    woocommerce_webhook_secret: PatchValue<WoocommerceWebhookSecret>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::currency::patch::deserialize")]
    woocommerce_currency: PatchValue<Currency>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::language::patch::deserialize")]
    woocommerce_language: PatchValue<Language>,
    #[serde(default)]
    url: PatchValue<Url>,
    #[serde(default)]
    image: PatchValue<Url>,
    #[serde(default)]
    structured_address: PatchValue<StructuredAddressData>,
    #[serde(default)]
    phone: PatchValue<String>,
    #[serde(default)]
    email: PatchValue<Email>,
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
    let command = match data.into_command(shop_id) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update_shop.execute(&context, command).await {
        Ok(view) => shop_response(view, None),
        Err(error) => ApiError::from(error).into_response(),
    }
}

impl UpdateShopData {
    fn into_command(self, shop_id: ShopId) -> Result<UpdateShopCommand, ApiError> {
        Ok(UpdateShopCommand {
            shop_id,
            shop_type: non_nullable_patch(self.shop_type, "shopType")?,
            domains: non_nullable_patch(self.domains, "domains")?,
            shopify_domain: clearable(self.shopify_domain),
            shopify_currency: clearable(self.shopify_currency),
            shopify_language: clearable(self.shopify_language),
            woocommerce_webhook_secret: clearable(self.woocommerce_webhook_secret),
            woocommerce_currency: clearable(self.woocommerce_currency),
            woocommerce_language: clearable(self.woocommerce_language),
            url: clearable(self.url),
            image: clearable(self.image),
            structured_address: clearable(self.structured_address.map(Into::into)),
            phone: clearable(self.phone),
            email: clearable(self.email),
            affiliate_configuration: PatchField::Unchanged,
        })
    }
}

fn parse_body(body: &str) -> Result<UpdateShopData, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty"));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
