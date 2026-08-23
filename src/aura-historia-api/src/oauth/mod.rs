#![allow(clippy::result_large_err)]

pub mod authorize;
pub mod create_client;
pub mod delete_client;
pub mod get_client;
pub mod introspect;
pub mod list_clients;
pub mod revoke;
pub mod token;
pub mod token_by_third_party_code;
pub mod update_client;

use crate::error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, HashSet};
use user_core::access_token::Scope;

pub(crate) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(crate) fn scope_strings(scopes: HashSet<Scope>) -> Vec<String> {
    let mut values = scopes
        .into_iter()
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values
}

pub(crate) fn scope_string(scopes: &HashSet<Scope>) -> String {
    let mut values = scopes
        .iter()
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values.join(" ")
}

pub(crate) fn parse_scopes(
    values: impl IntoIterator<Item = String>,
) -> Result<HashSet<Scope>, Response> {
    values
        .into_iter()
        .map(|value| parse_scope(&value))
        .collect()
}

pub(crate) fn parse_scope_string(
    value: Option<&str>,
    field: &'static str,
) -> Result<HashSet<Scope>, Response> {
    value
        .unwrap_or("")
        .split_whitespace()
        .map(|scope| parse_scope_with_field(scope, field, true))
        .collect()
}

fn parse_scope(value: &str) -> Result<Scope, Response> {
    parse_scope_with_field(value, "scope", false)
}

fn parse_scope_with_field(
    value: &str,
    field: &'static str,
    query: bool,
) -> Result<Scope, Response> {
    let scope = match value {
        "products:write" => Scope::ProductsWrite,
        "shops:read" => Scope::ShopsRead,
        "shops:write" => Scope::ShopsWrite,
        "partner-shop-applications:write" => Scope::PartnerShopApplicationsWrite,
        "partner-shops:read" => Scope::PartnerShopsRead,
        "partner-shops:write" => Scope::PartnerShopsWrite,
        "users:read" => Scope::UsersRead,
        "users:write" => Scope::UsersWrite,
        "access-tokens:read" => Scope::AccessTokensRead,
        "access-tokens:write" => Scope::AccessTokensWrite,
        "search-filters:write" => Scope::SearchFiltersWrite,
        "watchlist:read" => Scope::WatchlistRead,
        "watchlist:write" => Scope::WatchlistWrite,
        _ => {
            let error = ApiError::bad_request(if query {
                BAD_QUERY_PARAMETER_VALUE
            } else {
                BAD_BODY_VALUE
            })
            .with_detail(format!("Unsupported scope '{value}'."));
            return Err(if query {
                error.with_query_field(field)
            } else {
                error
            }
            .into_response());
        }
    };
    Ok(scope)
}

pub(crate) fn parse_form(body: String) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect()
}

pub(crate) fn required_form<'a>(
    form: &'a HashMap<String, String>,
    field: &'static str,
) -> Result<&'a str, Response> {
    form.get(field)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(format!("Form field '{field}' is required."))
                .into_response()
        })
}
