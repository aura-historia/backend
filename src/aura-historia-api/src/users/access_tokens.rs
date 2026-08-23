use super::util::{no_store, parse_json};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::state::UsersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope};
use user_core::user_id::UserId;
use user_service::use_cases::commands::create_access_token::{
    CreateAccessTokenCommand, CreateAccessTokenResult,
};
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenCommand;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenCommand;
use user_service::use_cases::queries::get_access_token::{AccessTokenView, GetAccessTokenRequest};
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensRequest;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostTokenData {
    name: String,
    scopes: HashSet<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    expires: Option<OffsetDateTime>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchTokenData {
    access_token_id: AccessTokenId,
    #[serde(default)]
    name: PatchValue<String>,
    #[serde(default)]
    scopes: PatchValue<HashSet<String>>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    expires: PatchValue<OffsetDateTime>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenData {
    user_id: UserId,
    access_token_id: AccessTokenId,
    name: String,
    scopes: Vec<String>,
    origin: String,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    expires: Option<OffsetDateTime>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTokenData {
    user_id: UserId,
    access_token_id: AccessTokenId,
    access_token: String,
}
impl From<AccessTokenView> for TokenData {
    fn from(v: AccessTokenView) -> Self {
        Self {
            user_id: v.user_id,
            access_token_id: v.access_token_id,
            name: v.name.to_string(),
            scopes: v
                .scopes
                .into_iter()
                .map(|s| s.as_str().to_owned())
                .collect(),
            origin: format!("{:?}", v.origin),
            expires: v.expires,
        }
    }
}
impl From<CreateAccessTokenResult> for CreatedTokenData {
    fn from(v: CreateAccessTokenResult) -> Self {
        Self {
            user_id: v.user_id,
            access_token_id: v.access_token_id,
            access_token: String::from(v.raw_access_token),
        }
    }
}

pub async fn post_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PostTokenData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = CreateAccessTokenCommand {
        user_id,
        name: AccessTokenName::from(data.name.as_str()),
        scopes: parse_scopes(data.scopes),
        expires: data.expires,
        origin: AccessTokenOrigin::User,
    };
    match state.create_access_token.execute(&ctx, command).await {
        Ok(r) => (StatusCode::CREATED, Json(CreatedTokenData::from(r))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn list_access_tokens(State(state): State<UsersState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match state
        .list_access_tokens
        .execute(&ctx, ListAccessTokensRequest { user_id })
        .await
    {
        Ok(r) => no_store(
            Json(r.items.into_iter().map(TokenData::from).collect::<Vec<_>>()).into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn get_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let access_token_id = match AccessTokenId::try_from(raw.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("accessTokenId")
                .with_detail("Path parameter 'accessTokenId' must be a UUID.")
                .into_response();
        }
    };
    match state
        .get_access_token
        .execute(
            &ctx,
            GetAccessTokenRequest {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(r) => no_store(Json(TokenData::from(r)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn patch_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PatchTokenData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };
    match state.update_access_token.execute(&ctx, command).await {
        Ok(result) => no_store(Json(TokenData::from(result.view)).into_response()),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn delete_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let access_token_id = match AccessTokenId::try_from(raw.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("accessTokenId")
                .with_detail("Path parameter 'accessTokenId' must be a UUID.")
                .into_response();
        }
    };
    match state
        .delete_access_token
        .execute(
            &ctx,
            DeleteAccessTokenCommand {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
impl PatchTokenData {
    fn into_command(self, user_id: UserId) -> Result<UpdateAccessTokenCommand, ApiError> {
        Ok(UpdateAccessTokenCommand {
            user_id,
            access_token_id: self.access_token_id,
            name: non_nullable_patch(
                self.name.map(|name| AccessTokenName::from(name.as_str())),
                "name",
            )?,
            scopes: non_nullable_patch(self.scopes.map(parse_scopes), "scopes")?,
            expires: clearable(self.expires),
        })
    }
}

fn parse_scopes(values: HashSet<String>) -> HashSet<Scope> {
    values
        .into_iter()
        .filter_map(|v| match v.as_str() {
            "products:write" => Some(Scope::ProductsWrite),
            "shops:read" => Some(Scope::ShopsRead),
            "shops:write" => Some(Scope::ShopsWrite),
            "partner-shop-applications:write" => Some(Scope::PartnerShopApplicationsWrite),
            "partner-shops:read" => Some(Scope::PartnerShopsRead),
            "partner-shops:write" => Some(Scope::PartnerShopsWrite),
            "users:read" => Some(Scope::UsersRead),
            "users:write" => Some(Scope::UsersWrite),
            "access-tokens:read" => Some(Scope::AccessTokensRead),
            "access-tokens:write" => Some(Scope::AccessTokensWrite),
            "search-filters:write" => Some(Scope::SearchFiltersWrite),
            "watchlist:read" => Some(Scope::WatchlistRead),
            "watchlist:write" => Some(Scope::WatchlistWrite),
            _ => None,
        })
        .collect()
}
