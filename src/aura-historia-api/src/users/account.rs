use super::types::{PatchUserData, UserData};
use super::util::{no_store, parse_json, parse_role, parse_tier, patch};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::UsersState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use common::patch_field::PatchField;
use common::user_id::UserId;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::commands::update_user::UpdateUserCommand;
use user_service::use_cases::queries::get_user::GetUserRequest;

pub async fn get_me(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .get_user
        .execute(&ctx, GetUserRequest::ById(user_id))
        .await
    {
        Ok(view) => no_store(Json(UserData::from(view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn patch_me(
    State(state): State<UsersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    patch_user(state, ctx, user_id, body).await
}

pub async fn delete_me(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .delete_user
        .execute(&ctx, DeleteUserCommand { user_id })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}
pub(crate) async fn patch_user(
    state: UsersState,
    ctx: common::operation_context::OperationContext,
    user_id: UserId,
    body: String,
) -> Response {
    let data: PatchUserData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = UpdateUserCommand {
        user_id,
        email: patch(data.email),
        first_name: patch(data.first_name),
        last_name: patch(data.last_name),
        language: patch(data.language.map(Into::into)),
        currency: patch(data.currency.map(Into::into)),
        measurement_unit: patch(data.measurement_unit.map(Into::into)),
        prohibited_content_consent: patch(data.prohibited_content_consent),
        tier: patch(data.tier.and_then(parse_tier)),
        role: patch(data.role.and_then(parse_role)),
        stripe_customer_id: PatchField::Unchanged,
        structured_address: patch(data.structured_address.map(Into::into)),
    };
    match state.update_user.execute(&ctx, command).await {
        Ok(result) => match state
            .get_user
            .execute(&ctx, GetUserRequest::ById(result.user_id))
            .await
        {
            Ok(view) => no_store(Json(UserData::from(view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        },
        Err(error) => ApiError::from(error).into_response(),
    }
}
