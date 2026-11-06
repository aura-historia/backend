use super::util::{no_store, parse_json, parse_user_id};
use crate::auth::protected_context;
use crate::error::{
    ACCESS_TOKEN_INTERNAL_ERROR, ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE,
};
use crate::pagination_data::JsonCursoredData;
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::state::UsersState;
use crate::wire::{parse_body_object_id, parse_path_object_id, parse_query_object_id};
use application::pagination::{Cursor, CursoredResult};
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_service::use_cases::DisconnectOAuthClientCommand;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::access_token::{
    AccessTokenId, AccessTokenName, AccessTokenOrigin, OAuthConnectionId, Scope,
};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenListEntry, AccessTokenListEntryId, ListedAccessToken, ListedOAuthConnection,
};
use user_service::use_cases::commands::create_access_token::{
    CreateAccessTokenCommand, CreateAccessTokenResult,
};
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenCommand;
use user_service::use_cases::commands::delete_access_tokens::DeleteAccessTokensCommand;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenCommand;
use user_service::use_cases::queries::get_access_token::{AccessTokenView, GetAccessTokenRequest};
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensRequest;
use user_service::use_cases::queries::list_admin_access_tokens::{
    AccessTokenSearchCursor, ListAdminAccessTokensRequest, ListAdminAccessTokensResult,
};

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
    access_token_id: String,
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
    origin: &'static str,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    expires: Option<OffsetDateTime>,
}

/// One entry of the credential listing: a token the user manages individually, or a
/// metadata document client's connection, which stands for its rotating tokens.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccessTokenListEntryData {
    user_id: UserId,
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token_id: Option<AccessTokenId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_id: Option<OAuthConnectionId>,
    name: String,
    scopes: Vec<String>,
    origin: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<OAuthClientData>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    expires: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    created: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    last_used: Option<OffsetDateTime>,
}

/// The OAuth client holding a listed credential.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OAuthClientData {
    client_id: String,
    name: String,
    kind: &'static str,
    host: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisconnectQuery {
    #[serde(default)]
    client_id: Option<String>,
}

/// Stable wire values of a credential's origin. `User` keeps the value listings always had.
fn origin_value(origin: &AccessTokenOrigin) -> &'static str {
    match origin {
        AccessTokenOrigin::User => "User",
        AccessTokenOrigin::OAuth { .. } => "OAuth",
        AccessTokenOrigin::OAuthMetadataDocument { .. } => "OAuthMetadataDocument",
    }
}

fn scope_values(scopes: HashSet<Scope>) -> Vec<String> {
    let mut values = scopes
        .into_iter()
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values
}

impl From<AccessTokenListEntry> for AccessTokenListEntryData {
    fn from(entry: AccessTokenListEntry) -> Self {
        match entry {
            AccessTokenListEntry::AccessToken(listed) => Self::from(listed),
            AccessTokenListEntry::Connection(connection) => Self::from(connection),
        }
    }
}

impl From<ListedAccessToken> for AccessTokenListEntryData {
    fn from(listed: ListedAccessToken) -> Self {
        let ListedAccessToken { token, client } = listed;
        let client = match (&token.origin, client) {
            (AccessTokenOrigin::OAuth { client_id }, Some(client)) => Some(OAuthClientData {
                client_id: client_id.to_string(),
                name: client.name,
                kind: "REGISTERED",
                host: client.host,
            }),
            _ => None,
        };
        Self {
            user_id: token.user_id,
            access_token_id: Some(token.access_token_id),
            connection_id: None,
            name: token.name.to_string(),
            scopes: scope_values(token.scopes),
            origin: origin_value(&token.origin),
            client,
            expires: token.expires,
            created: None,
            last_used: None,
        }
    }
}

impl From<ListedOAuthConnection> for AccessTokenListEntryData {
    fn from(connection: ListedOAuthConnection) -> Self {
        Self {
            user_id: connection.user_id,
            access_token_id: None,
            connection_id: Some(connection.connection_id),
            name: connection.client_name.clone(),
            scopes: scope_values(connection.scopes),
            origin: "OAuthMetadataDocument",
            client: Some(OAuthClientData {
                host: connection.client_id.host().to_owned(),
                client_id: connection.client_id.into(),
                name: connection.client_name,
                kind: "METADATA_DOCUMENT",
            }),
            expires: None,
            created: Some(connection.created),
            last_used: Some(connection.last_used),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminAccessTokensQuery {
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

const DEFAULT_PAGE_SIZE: u64 = 21;
const MAX_PAGE_SIZE: u64 = 100;

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
            origin: origin_value(&v.origin),
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
    let scopes = match parse_scopes(data.scopes) {
        Ok(scopes) => scopes,
        Err(error) => return error.into_response(),
    };
    let command = CreateAccessTokenCommand {
        user_id,
        name: AccessTokenName::from(data.name.as_str()),
        scopes,
        expires: data.expires,
        origin: AccessTokenOrigin::User,
    };
    match state.create_access_token.execute(&ctx, command).await {
        Ok(r) => no_store((StatusCode::CREATED, Json(CreatedTokenData::from(r))).into_response()),
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
            Json(
                r.items
                    .into_iter()
                    .map(AccessTokenListEntryData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}
pub async fn list_admin_access_tokens(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(value) => value,
        Err(response) => return no_store(response),
    };
    let request = match parse_list_admin_access_tokens_query(user_id, raw_query.as_deref()) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.admin_list_access_tokens.execute(&ctx, request).await {
        Ok(result) => match response_from_admin_result(result) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_admin_access_tokens_query(
    user_id: UserId,
    raw_query: Option<&str>,
) -> Result<ListAdminAccessTokensRequest, ApiError> {
    let query: ListAdminAccessTokensQuery =
        crate::wire::from_query_str(raw_query.unwrap_or_default())
            .map_err(|error| bad_query("query", error))?;
    let size = query
        .size
        .map(|value| {
            value
                .parse::<u64>()
                .map(|size| size.clamp(1, MAX_PAGE_SIZE))
                .map_err(|error| bad_query("size", error))
        })
        .transpose()?;
    let search_after = query
        .search_after
        .as_deref()
        .map(parse_access_token_search_after)
        .transpose()?;

    Ok(ListAdminAccessTokensRequest {
        user_id,
        cursor: if size.is_some() || search_after.is_some() {
            Some(Cursor {
                size: size.unwrap_or(DEFAULT_PAGE_SIZE),
                search_after,
            })
        } else {
            None
        },
    })
}

fn parse_access_token_search_after(value: &str) -> Result<AccessTokenSearchCursor, ApiError> {
    let value: Value = serde_json::from_str(value).map_err(|error| {
        bad_query(
            "searchAfter",
            format!(
                "searchAfter must be a JSON array containing timestamp and AccessToken ID: {error}"
            ),
        )
    })?;
    let Value::Array(values) = value else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and AccessToken ID.",
        ));
    };
    let [Value::String(position), Value::String(access_token_id)] = values.as_slice() else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and AccessToken ID.",
        ));
    };
    let position = OffsetDateTime::parse(position, &Rfc3339)
        .map_err(|error| bad_query("searchAfter", error))?;
    let entry_id =
        match parse_query_object_id::<AccessTokenId>(access_token_id, "searchAfter", "AccessToken")
        {
            Ok(access_token_id) => AccessTokenListEntryId::AccessToken(access_token_id),
            Err(error) => access_token_id
                .parse::<OAuthConnectionId>()
                .map(AccessTokenListEntryId::Connection)
                .map_err(|_| error)?,
        };

    Ok(AccessTokenSearchCursor { position, entry_id })
}

fn response_from_admin_result(
    result: ListAdminAccessTokensResult,
) -> Result<JsonCursoredData<AccessTokenListEntryData>, ApiError> {
    let CursoredResult {
        items,
        cursor,
        total,
    } = result;
    let search_after = cursor
        .search_after
        .map(serialize_access_token_search_after)
        .transpose()?;

    Ok(JsonCursoredData::new(
        items
            .into_iter()
            .map(AccessTokenListEntryData::from)
            .collect(),
        search_after,
        total,
    ))
}

fn serialize_access_token_search_after(cursor: AccessTokenSearchCursor) -> Result<Value, ApiError> {
    let position = cursor
        .position
        .format(&Rfc3339)
        .map_err(|_| ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR))?;
    let entry_id = match cursor.entry_id {
        AccessTokenListEntryId::AccessToken(id) => id.to_string(),
        AccessTokenListEntryId::Connection(id) => id.to_string(),
    };
    Ok(json!([position, entry_id]))
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
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
    let access_token_id = match parse_path_object_id(&raw, "accessTokenId", "AccessToken") {
        Ok(value) => value,
        Err(error) => return error.into_response(),
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
    let access_token_id = match parse_path_object_id(&raw, "accessTokenId", "AccessToken") {
        Ok(value) => value,
        Err(error) => return error.into_response(),
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

/// Disconnects one OAuth client: revokes every credential the user granted it.
pub async fn delete_access_tokens(
    State(state): State<UsersState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let client_id = match parse_disconnect_query(raw_query.as_deref()) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };
    match state
        .disconnect_oauth_client
        .execute(&ctx, DisconnectOAuthClientCommand { user_id, client_id })
        .await
    {
        Ok(()) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_disconnect_query(raw_query: Option<&str>) -> Result<OAuthClientIdentifier, ApiError> {
    let query: DisconnectQuery = crate::wire::from_query_str(raw_query.unwrap_or_default())
        .map_err(|error| bad_query("query", error))?;
    query
        .client_id
        .as_deref()
        .map(OAuthClientIdentifier::parse)
        .and_then(Result::ok)
        .ok_or_else(|| {
            bad_query(
                "clientId",
                "clientId must be a registered OAuth client ID or a client metadata document URL.",
            )
        })
}

pub async fn delete_admin_access_tokens(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path(raw_user_id): Path<String>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };

    match state
        .admin_delete_access_tokens
        .execute(&ctx, DeleteAccessTokensCommand { user_id })
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

pub async fn delete_admin_access_token(
    State(state): State<UsersState>,
    headers: HeaderMap,
    Path((raw_user_id, raw_access_token_id)): Path<(String, String)>,
) -> Response {
    let (ctx, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return no_store(*r),
    };
    let user_id = match parse_user_id(&raw_user_id, "userId") {
        Ok(v) => v,
        Err(r) => return no_store(r),
    };
    let access_token_id =
        match parse_path_object_id(&raw_access_token_id, "accessTokenId", "AccessToken") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };

    match state
        .admin_delete_access_token
        .execute(
            &ctx,
            DeleteAccessTokenCommand {
                user_id,
                access_token_id,
            },
        )
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

impl PatchTokenData {
    fn into_command(self, user_id: UserId) -> Result<UpdateAccessTokenCommand, ApiError> {
        Ok(UpdateAccessTokenCommand {
            user_id,
            access_token_id: parse_body_object_id(
                &self.access_token_id,
                "accessTokenId",
                "AccessToken",
            )?,
            name: non_nullable_patch(
                self.name.map(|name| AccessTokenName::from(name.as_str())),
                "name",
            )?,
            scopes: non_nullable_patch(parse_scope_patch(self.scopes)?, "scopes")?,
            expires: clearable(self.expires),
        })
    }
}

fn parse_scope_patch(
    values: PatchValue<HashSet<String>>,
) -> Result<PatchValue<HashSet<Scope>>, ApiError> {
    match values {
        PatchValue::Omitted => Ok(PatchValue::Omitted),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Value(values) => Ok(PatchValue::Value(parse_scopes(values)?)),
    }
}

fn parse_scopes(values: HashSet<String>) -> Result<HashSet<Scope>, ApiError> {
    values
        .into_iter()
        .map(|value| match value.as_str() {
            "product-listings:write" => Ok(Scope::ProductListingsWrite),
            "listing-sources:write" => Ok(Scope::ListingSourcesWrite),
            "auctions:read" => Ok(Scope::AuctionsRead),
            "product-listings:read" => Ok(Scope::ProductListingsRead),
            "auctions:write" => Ok(Scope::AuctionsWrite),
            "listing-sources:read" => Ok(Scope::ListingSourcesRead),
            "parties:read" => Ok(Scope::PartiesRead),
            "parties:write" => Ok(Scope::PartiesWrite),
            "partnership-applications:read" => Ok(Scope::PartnershipApplicationsRead),
            "partnership-applications:write" => Ok(Scope::PartnershipApplicationsWrite),
            "partnerships:read" => Ok(Scope::PartnershipsRead),
            "partnerships:write" => Ok(Scope::PartnershipsWrite),
            "admin-overview:read" => Ok(Scope::AdminOverviewRead),
            "search-filters:read" => Ok(Scope::SearchFiltersRead),
            "notifications:read" => Ok(Scope::NotificationsRead),
            "notifications:write" => Ok(Scope::NotificationsWrite),
            "users:read" => Ok(Scope::UsersRead),
            "users:write" => Ok(Scope::UsersWrite),
            "access-tokens:read" => Ok(Scope::AccessTokensRead),
            "access-tokens:write" => Ok(Scope::AccessTokensWrite),
            "search-filters:write" => Ok(Scope::SearchFiltersWrite),
            "watchlist:read" => Ok(Scope::WatchlistRead),
            "watchlist:write" => Ok(Scope::WatchlistWrite),
            _ => Err(ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(format!("Unsupported scope '{}'.", value))),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_admin_access_token_query_with_clamped_cursor() -> Result<(), ApiError> {
        let user_id = UserId::new();
        let access_token_id = AccessTokenId::new();
        let request = parse_list_admin_access_tokens_query(
            user_id,
            Some(&format!(
                "size=200&searchAfter=[\"2026-09-04T12:00:00Z\",\"{access_token_id}\"]"
            )),
        )?;

        assert_eq!(user_id, request.user_id);
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(AccessTokenSearchCursor {
                    position: OffsetDateTime::parse("2026-09-04T12:00:00Z", &Rfc3339,).map_err(
                        |error| ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                            .with_detail(error.to_string())
                    )?,
                    entry_id: AccessTokenListEntryId::AccessToken(access_token_id),
                }),
            }),
            request.cursor
        );
        let connection_id = OAuthConnectionId::new();
        let request = parse_list_admin_access_tokens_query(
            user_id,
            Some(&format!(
                "searchAfter=[\"2026-09-04T12:00:00Z\",\"{connection_id}\"]"
            )),
        )?;
        assert_eq!(
            Some(AccessTokenListEntryId::Connection(connection_id)),
            request
                .cursor
                .and_then(|cursor| cursor.search_after)
                .map(|search_after| search_after.entry_id)
        );
        Ok(())
    }

    #[test]
    fn should_reject_invalid_admin_access_token_query_values() {
        for query in [
            "size=not-a-number",
            "searchAfter=not-json",
            "searchAfter=%5B%22not-a-timestamp%22%2C%22not-an-object-id%22%5D",
        ] {
            assert!(
                parse_list_admin_access_tokens_query(UserId::new(), Some(query)).is_err(),
                "{query}"
            );
        }
    }

    #[test]
    fn should_serialize_own_access_token_metadata_with_scopes_and_expiry() {
        let value = serde_json::to_value(TokenData::from(AccessTokenView {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            name: AccessTokenName::from("own token"),
            scopes: HashSet::from([Scope::UsersRead, Scope::UsersWrite]),
            origin: AccessTokenOrigin::User,
            expires: Some(
                OffsetDateTime::parse("2026-06-30T00:00:00Z", &Rfc3339)
                    .unwrap_or_else(|error| panic!("test expiry is valid RFC3339: {error}")),
            ),
        }))
        .unwrap_or_else(|error| panic!("own access-token metadata serializes: {error}"));

        let scopes = value["scopes"]
            .as_array()
            .unwrap_or_else(|| panic!("scopes serialize as an array: {value}"))
            .iter()
            .map(|scope| {
                scope
                    .as_str()
                    .unwrap_or_else(|| panic!("scope serializes as a string: {scope}"))
            })
            .collect::<HashSet<_>>();
        assert_eq!(HashSet::from(["users:read", "users:write"]), scopes);
        assert_eq!("2026-06-30T00:00:00Z", value["expires"]);
        assert_eq!("User", value["origin"]);
        for (origin, expected) in [
            (
                AccessTokenOrigin::OAuth {
                    client_id: user_core::access_token::OAuthClientId::new(),
                },
                "OAuth",
            ),
            (
                AccessTokenOrigin::OAuthMetadataDocument {
                    client_id: "https://agent.example/client.json".parse().unwrap(),
                    connection_id: OAuthConnectionId::new(),
                },
                "OAuthMetadataDocument",
            ),
        ] {
            assert_eq!(expected, origin_value(&origin));
        }
        assert!(value.get("accessTokenId").is_some());
        for field in ["accessToken", "token", "tokenShort", "tokenHash", "hash"] {
            assert!(value.get(field).is_none(), "unexpected {field}: {value}");
        }
    }

    #[test]
    fn should_serialize_own_access_token_metadata_with_empty_scopes_and_no_expiry() {
        let value = serde_json::to_value(TokenData::from(AccessTokenView {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            name: AccessTokenName::from("unscoped token"),
            scopes: HashSet::new(),
            origin: AccessTokenOrigin::User,
            expires: None,
        }))
        .unwrap_or_else(|error| panic!("own access-token metadata serializes: {error}"));

        assert_eq!(json!([]), value["scopes"]);
        assert!(value.get("expires").is_none());
        assert!(value.get("accessToken").is_none());
        assert!(value.get("token").is_none());
    }

    #[test]
    fn should_deserialize_access_token_creation_with_required_scopes_and_optional_expiry() {
        let selected_scopes: PostTokenData = serde_json::from_value(json!({
            "name": "partner sync",
            "scopes": ["users:read", "users:write"],
            "expires": "2026-06-30T00:00:00Z"
        }))
        .unwrap_or_else(|error| panic!("selected-scope create request deserializes: {error}"));
        assert_eq!("partner sync", selected_scopes.name);
        assert_eq!(
            HashSet::from(["users:read".to_owned(), "users:write".to_owned()]),
            selected_scopes.scopes
        );
        assert_eq!(
            Some(
                OffsetDateTime::parse("2026-06-30T00:00:00Z", &Rfc3339)
                    .unwrap_or_else(|error| panic!("test expiry is valid RFC3339: {error}")),
            ),
            selected_scopes.expires
        );

        let empty_scopes: PostTokenData = serde_json::from_value(json!({
            "name": "unscoped token",
            "scopes": []
        }))
        .unwrap_or_else(|error| panic!("empty-scope create request deserializes: {error}"));
        assert!(empty_scopes.scopes.is_empty());
        assert_eq!(None, empty_scopes.expires);
        assert!(
            serde_json::from_value::<PostTokenData>(json!({"name": "missing scopes"})).is_err()
        );
    }

    #[test]
    fn should_serialize_create_result_with_only_identity_and_one_time_plaintext() {
        let value = serde_json::to_value(CreatedTokenData {
            user_id: UserId::new(),
            access_token_id: AccessTokenId::new(),
            access_token: "aurahistoria_one_time_secret".to_owned(),
        })
        .unwrap_or_else(|error| panic!("created access-token response serializes: {error}"));

        let fields = value
            .as_object()
            .unwrap_or_else(|| panic!("create result serializes as an object: {value}"))
            .keys()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        assert_eq!(
            HashSet::from(["userId", "accessTokenId", "accessToken"]),
            fields
        );
        assert_eq!("aurahistoria_one_time_secret", value["accessToken"]);
    }

    #[test]
    fn should_accept_canonical_product_listings_write_scope() {
        let scopes = parse_scopes(HashSet::from(["product-listings:write".to_owned()]));

        assert!(matches!(
            scopes,
            Ok(scopes) if scopes == HashSet::from([Scope::ProductListingsWrite])
        ));
    }

    #[test]
    fn should_accept_canonical_listing_sources_write_scope() {
        let scopes = parse_scopes(HashSet::from(["listing-sources:write".to_owned()]));

        assert!(matches!(
            scopes,
            Ok(scopes) if scopes == HashSet::from([Scope::ListingSourcesWrite])
        ));
    }

    #[test]
    fn should_accept_canonical_auctions_read_scope() {
        let scopes = parse_scopes(HashSet::from(["auctions:read".to_owned()]));

        assert!(matches!(
            scopes,
            Ok(scopes) if scopes == HashSet::from([Scope::AuctionsRead])
        ));
    }

    #[test]
    fn should_accept_canonical_product_listings_read_scope() {
        let scopes = parse_scopes(HashSet::from(["product-listings:read".to_owned()]));

        assert!(matches!(
            scopes,
            Ok(scopes) if scopes == HashSet::from([Scope::ProductListingsRead])
        ));
    }

    #[test]
    fn should_reject_noncanonical_auctions_read_scopes() {
        for value in [
            "auction:read",
            "AUCTIONS_READ",
            "auctions:READ",
            "auctions:delete",
        ] {
            assert!(
                parse_scopes(HashSet::from([value.to_owned()])).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn should_reject_unsupported_access_token_scope() {
        let scopes = parse_scopes(HashSet::from(["unsupported:scope".to_owned()]));

        assert!(scopes.is_err());
    }
    #[test]
    fn should_round_trip_new_credential_scopes() {
        for (scope, value) in [
            (Scope::AuctionsWrite, "auctions:write"),
            (Scope::ListingSourcesRead, "listing-sources:read"),
            (Scope::PartiesRead, "parties:read"),
            (Scope::PartiesWrite, "parties:write"),
            (
                Scope::PartnershipApplicationsRead,
                "partnership-applications:read",
            ),
            (
                Scope::PartnershipApplicationsWrite,
                "partnership-applications:write",
            ),
            (Scope::PartnershipsRead, "partnerships:read"),
            (Scope::PartnershipsWrite, "partnerships:write"),
            (Scope::AdminOverviewRead, "admin-overview:read"),
            (Scope::SearchFiltersRead, "search-filters:read"),
            (Scope::NotificationsRead, "notifications:read"),
            (Scope::NotificationsWrite, "notifications:write"),
        ] {
            assert_eq!(
                HashSet::from([scope]),
                parse_scopes(HashSet::from([value.to_owned()])).expect("canonical scope")
            );
            assert_eq!(value, scope.as_str());
        }
    }

    #[test]
    fn should_list_registered_client_tokens_and_connections_with_their_client_identity() {
        use user_service::ports::{AccessTokenDetails, RegisteredOAuthClientDisplay};
        let user_id = UserId::new();
        let client_id = user_core::access_token::OAuthClientId::new();
        let registered = serde_json::to_value(AccessTokenListEntryData::from(
            AccessTokenListEntry::AccessToken(
                ListedAccessToken::new(
                    AccessTokenDetails {
                        user_id,
                        access_token_id: AccessTokenId::new(),
                        name: AccessTokenName::from("Shop sync"),
                        scopes: HashSet::from([Scope::ProductListingsWrite]),
                        origin: AccessTokenOrigin::OAuth { client_id },
                        expires: None,
                    },
                    Some(RegisteredOAuthClientDisplay {
                        name: "Shop".to_owned(),
                        host: "shop.example".to_owned(),
                    }),
                )
                .unwrap(),
            ),
        ))
        .unwrap();
        assert_eq!("OAuth", registered["origin"]);
        assert_eq!(
            json!({
                "clientId": client_id.to_string(),
                "name": "Shop",
                "kind": "REGISTERED",
                "host": "shop.example",
            }),
            registered["client"]
        );
        assert!(registered.get("connectionId").is_none());

        let connection_id = OAuthConnectionId::new();
        let connection = serde_json::to_value(AccessTokenListEntryData::from(
            AccessTokenListEntry::Connection(ListedOAuthConnection {
                connection_id,
                user_id,
                client_id: "https://agent.example/client.json".parse().unwrap(),
                client_name: "Agent".to_owned(),
                scopes: HashSet::from([Scope::WatchlistRead, Scope::UsersRead]),
                created: OffsetDateTime::parse("2026-11-01T00:00:00Z", &Rfc3339).unwrap(),
                last_used: OffsetDateTime::parse("2026-11-06T12:00:00Z", &Rfc3339).unwrap(),
            }),
        ))
        .unwrap();
        assert_eq!(
            json!({
                "userId": user_id,
                "connectionId": connection_id,
                "name": "Agent",
                "scopes": ["users:read", "watchlist:read"],
                "origin": "OAuthMetadataDocument",
                "client": {
                    "clientId": "https://agent.example/client.json",
                    "name": "Agent",
                    "kind": "METADATA_DOCUMENT",
                    "host": "agent.example",
                },
                "created": "2026-11-01T00:00:00Z",
                "lastUsed": "2026-11-06T12:00:00Z",
            }),
            connection
        );
    }

    #[test]
    fn should_require_a_client_id_of_either_kind_to_disconnect() {
        let registered = user_core::access_token::OAuthClientId::new();
        assert!(matches!(
            parse_disconnect_query(Some(&format!("clientId={registered}"))),
            Ok(OAuthClientIdentifier::Registered(id)) if id == registered
        ));
        assert!(matches!(
            parse_disconnect_query(Some("clientId=https%3A%2F%2Fagent.example%2Fclient.json")),
            Ok(OAuthClientIdentifier::MetadataDocument(_))
        ));
        for query in [
            None,
            Some("clientId="),
            Some("clientId=https%3A%2F%2Fagent.example"),
            Some("clientId=not-a-client"),
        ] {
            assert!(parse_disconnect_query(query).is_err(), "{query:?}");
        }
    }
}
