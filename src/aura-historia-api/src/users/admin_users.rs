use super::account::patch_user;
use super::types::{CursorData, UserData, UserSummaryData};
use super::util::{no_store, parse_user_id};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::UsersState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use common::pagination::cursor::Cursor;
use std::collections::HashMap;
use user_core::user_search::UserSearch;
use user_service::use_cases::commands::delete_user::DeleteUserCommand;
use user_service::use_cases::queries::get_user::GetUserRequest;
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
                    .map(UserSummaryData::from)
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
        .get_user
        .execute(&ctx, GetUserRequest::AdminById(user_id))
        .await
    {
        Ok(view) => no_store(Json(UserData::from(view)).into_response()),
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
    patch_user(state, ctx, user_id, body).await
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
