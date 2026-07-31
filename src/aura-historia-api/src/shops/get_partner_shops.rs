use crate::error::ApiError;
use crate::shops::authz::protected_context;
use crate::shops::shop_data::ShopData;
use crate::state::ShopsState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use shop_service::use_cases::queries::get_shop::GetShopRequest;
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsRequest;

pub async fn get_partner_shops(State(state): State<ShopsState>, headers: HeaderMap) -> Response {
    let (context, user_id) = match protected_context(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let shop_ids = match state
        .list_user_partner_shops
        .execute(&context, ListUserPartnerShopsRequest { user_id })
        .await
    {
        Ok(result) => result.shop_ids,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let mut shops = Vec::with_capacity(shop_ids.len());
    for shop_id in shop_ids {
        match state
            .get_shop
            .execute(&context, GetShopRequest::ById(shop_id))
            .await
        {
            Ok(view) => shops.push(ShopData::from(view)),
            Err(error) => return ApiError::from(error).into_response(),
        }
    }
    Json(shops).into_response()
}
