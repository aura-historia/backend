use super::types::{OwnUserData, PatchOwnUserData};
use super::util::{no_store, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::UsersState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use user_core::user_id::UserId;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::queries::get_own_user::GetOwnUserRequest;

pub async fn get_me(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
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
        Err(r) => return *r,
    };
    patch_user(state, ctx, user_id, body).await
}

pub async fn delete_me(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
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
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(error) => return ApiError::from(error).into_response(),
    };
    match state.update_user_profile.execute(&ctx, command).await {
        Ok(result) => no_store(Json(OwnUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

impl From<aura_historia_contracts::users::account::NullSensitiveContentPreference> for ApiError {
    fn from(
        error: aura_historia_contracts::users::account::NullSensitiveContentPreference,
    ) -> Self {
        Self::bad_request(crate::error::BAD_BODY_VALUE).with_detail(error.to_string())
    }
}
