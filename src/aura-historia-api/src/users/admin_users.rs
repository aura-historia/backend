use super::types::{AdminUserData, AdminUserSummaryData, CursorData, PatchAdminUserData};
use super::util::{no_store, parse_json, parse_user_id, patch};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE};
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
        Err(r) => return r,
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
        Err(r) => return r,
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
        Err(r) => return r,
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
    let profile_changed = data.email.is_some()
        || data.first_name.is_some()
        || data.last_name.is_some()
        || data.language.is_some()
        || data.currency.is_some()
        || data.measurement_unit.is_some()
        || data.prohibited_content_consent.is_some()
        || data.structured_address.is_some();
    let role_changed = data.role.is_some();
    let tier_changed = data.tier.is_some();
    let change_count = u8::from(profile_changed) + u8::from(role_changed) + u8::from(tier_changed);
    if change_count > 1 {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Patch only one user change category per request.")
            .into_response();
    }

    if let Some(role) = data.role {
        return match state
            .change_user_role
            .execute(
                &ctx,
                ChangeUserRoleCommand {
                    user_id,
                    role: role.into(),
                },
            )
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

    if let Some(tier) = data.tier {
        return match state
            .change_user_tier
            .execute(
                &ctx,
                ChangeUserTierCommand {
                    user_id,
                    tier: tier.into(),
                },
            )
            .await
        {
            Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
            Err(error) => ApiError::from(error).into_response(),
        };
    }

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
        Ok(result) => no_store(Json(AdminUserData::from(result.view)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn delete_admin_user(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
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
