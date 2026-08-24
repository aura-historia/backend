use super::types::{OwnUserData, PatchOwnUserData};
use super::util::{no_store, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::patch_value::{clearable, non_nullable_patch};
use crate::state::UsersState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use user_core::user_id::UserId;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand;
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
    let command = match into_command(data, user_id) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update_user_profile.execute(&ctx, command).await {
        Ok(result) => no_store(Json(OwnUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn into_command(
    data: PatchOwnUserData,
    user_id: UserId,
) -> Result<UpdateUserProfileCommand, ApiError> {
    Ok(UpdateUserProfileCommand {
        user_id,
        email: non_nullable_patch(data.email, "email")?,
        first_name: clearable(data.first_name),
        last_name: clearable(data.last_name),
        language: clearable(data.language),
        currency: clearable(data.currency),
        measurement_unit: clearable(data.measurement_unit),
        prohibited_content_consent: non_nullable_patch(
            data.prohibited_content_consent,
            "prohibitedContentConsent",
        )?,
        structured_address: clearable(data.structured_address.map(Into::into)),
    })
}
