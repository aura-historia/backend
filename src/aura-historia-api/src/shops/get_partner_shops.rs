use crate::error::ApiError;
use crate::shops::authz::protected_context;
use crate::shops::shop_data::ShopSummaryData;
use crate::state::ShopsState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use shop_service::use_cases::queries::list_user_partner_shops::ListUserPartnerShopsRequest;

pub async fn get_partner_shops(State(state): State<ShopsState>, headers: HeaderMap) -> Response {
    let (context, user_id) = match protected_context(&state, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .list_user_partner_shops
        .execute(&context, ListUserPartnerShopsRequest { user_id })
        .await
    {
        Ok(result) => Json(
            result
                .items
                .into_iter()
                .map(ShopSummaryData::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
