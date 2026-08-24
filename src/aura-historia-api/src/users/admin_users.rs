use super::types::{
    AdminUserData, AdminUserSummaryData, CursorData, PatchAdminUserData, PatchOwnUserData,
};
use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::patch_value::{clearable, non_nullable_option, non_nullable_patch};
use crate::state::UsersState;
use application::pagination::Cursor;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use user_core::user_search::UserSearch;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleCommand;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierCommand;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileCommand;
use user_service::use_cases::queries::admin_get_user::AdminGetUserRequest;
use user_service::use_cases::queries::search_users::SearchUsersRequest;

pub async fn search_users(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Query(_query): Query<HashMap<String, String>>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let request = SearchUsersRequest {
        search: UserSearch::default(),
        sort: None,
        cursor: Some(Cursor {
            search_after: None,
            size: 21,
        }),
    };
    match state.search_users.execute(&ctx, request).await {
        Ok(result) => no_store(
            Json(CursorData {
                items: result
                    .items
                    .into_iter()
                    .map(AdminUserSummaryData::from)
                    .collect(),
                size: result.cursor.size,
                search_after: result.cursor.search_after,
                total: result.total,
            })
            .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn get_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .admin_get_user
        .execute(&ctx, AdminGetUserRequest { user_id })
        .await
    {
        Ok(view) => no_store(Json(AdminUserData::from(view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn patch_admin_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let data: PatchAdminUserData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    admin_patch_user(state, ctx, user_id, data).await
}

async fn admin_patch_user(
    state: UsersState,
    ctx: application::operation_context::OperationContext,
    user_id: user_core::user_id::UserId,
    data: PatchAdminUserData,
) -> Response {
    let profile_changed = data.email.is_present()
        || data.first_name.is_present()
        || data.last_name.is_present()
        || data.language.is_present()
        || data.currency.is_present()
        || data.measurement_unit.is_present()
        || data.prohibited_content_consent.is_present()
        || data.structured_address.is_present();
    let role_changed = data.role.is_present();
    let tier_changed = data.tier.is_present();
    let change_count = u8::from(profile_changed) + u8::from(role_changed) + u8::from(tier_changed);
    if change_count > 1 {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Patch only one user change category per request.")
            .into_response();
    }

    let PatchAdminUserData {
        email,
        first_name,
        last_name,
        language,
        currency,
        measurement_unit,
        prohibited_content_consent,
        tier,
        role,
        structured_address,
    } = data;
    let role = match non_nullable_option(role, "role") {
        Ok(role) => role,
        Err(error) => return error.into_response(),
    };
    let tier = match non_nullable_option(tier, "tier") {
        Ok(tier) => tier,
        Err(error) => return error.into_response(),
    };

    if let Some(role) = role {
        return match state
            .change_user_role
            .execute(&ctx, ChangeUserRoleCommand { user_id, role })
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

    if let Some(tier) = tier {
        return match state
            .change_user_tier
            .execute(&ctx, ChangeUserTierCommand { user_id, tier })
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

    let command = match profile_command(
        PatchOwnUserData {
            email,
            first_name,
            last_name,
            language,
            currency,
            measurement_unit,
            prohibited_content_consent,
            structured_address,
        },
        user_id,
    ) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update_user_profile.execute(&ctx, command).await {
        Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn profile_command(
    data: PatchOwnUserData,
    user_id: user_core::user_id::UserId,
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

pub async fn delete_admin_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
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
