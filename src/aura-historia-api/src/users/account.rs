use super::types::{OwnUserData, PatchOwnUserData};
use super::util::{no_store, parse_json, patch};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::UsersState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use common::user_id::UserId;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand;
use user_service::use_cases::queries::get_own_user::GetOwnUserRequest;

pub async fn get_me(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state.get_own_user.execute(&ctx, GetOwnUserRequest).await {
        Ok(view) => no_store(Json(OwnUserData::from(view)).into_response()),
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
    ctx: application::operation_context::OperationContext,
    user_id: UserId,
    body: String,
) -> Response {
    let data: PatchOwnUserData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = UpdateUserProfileCommand {
        user_id,
        email: patch(data.email),
        first_name: patch(data.first_name),
        last_name: patch(data.last_name),
        language: patch(data.language.map(Into::into)),
        currency: patch(data.currency.map(Into::into)),
        measurement_unit: patch(data.measurement_unit.map(Into::into)),
        prohibited_content_consent: patch(data.prohibited_content_consent),
        structured_address: patch(data.structured_address.map(Into::into)),
    };
    match state.update_user_profile.execute(&ctx, command).await {
        Ok(result) => no_store(Json(OwnUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
