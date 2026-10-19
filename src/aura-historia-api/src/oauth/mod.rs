#![allow(clippy::result_large_err)]

pub mod authorize;
pub mod create_client;
pub mod delete_client;
pub mod get_client;
pub mod get_consent_client;
pub mod introspect;
pub mod list_clients;
pub mod revoke;
pub mod token;
pub mod token_by_third_party_code;
pub mod update_client;

use crate::error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, INVALID_CREDENTIALS};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, HashSet};
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret, Scope};

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
        "product-listings:write" => Scope::ProductListingsWrite,
        "listing-sources:write" => Scope::ListingSourcesWrite,
        "auctions:read" => Scope::AuctionsRead,
        "product-listings:read" => Scope::ProductListingsRead,
        "auctions:write" => Scope::AuctionsWrite,
        "listing-sources:read" => Scope::ListingSourcesRead,
        "parties:read" => Scope::PartiesRead,
        "parties:write" => Scope::PartiesWrite,
        "partnership-applications:read" => Scope::PartnershipApplicationsRead,
        "partnership-applications:write" => Scope::PartnershipApplicationsWrite,
        "partnerships:read" => Scope::PartnershipsRead,
        "partnerships:write" => Scope::PartnershipsWrite,
        "admin-overview:read" => Scope::AdminOverviewRead,
        "search-filters:read" => Scope::SearchFiltersRead,
        "notifications:read" => Scope::NotificationsRead,
        "notifications:write" => Scope::NotificationsWrite,
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

// Credential parse failures must not echo the submitted value, which may be a
// real secret sent in the wrong field.
pub(crate) fn required_form_access_token(
    form: &HashMap<String, String>,
) -> Result<RawAccessToken, Response> {
    RawAccessToken::try_from(required_form(form, "token")?.to_owned()).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_body_field("token")
            .with_detail("Form field 'token' is not a valid access token.")
            .into_response()
    })
}

pub(crate) fn required_form_client_secret(
    form: &HashMap<String, String>,
) -> Result<RawOAuthClientSecret, Response> {
    RawOAuthClientSecret::try_from(required_form(form, "client_secret")?.to_owned()).map_err(
        |_| {
            ApiError::unauthorized(INVALID_CREDENTIALS)
                .with_body_field("client_secret")
                .with_detail("Form field 'client_secret' is not a valid OAuth client secret.")
                .into_response()
        },
    )
}

#[cfg(test)]
pub(crate) mod test_support {
    use axum::response::Response;

    pub(crate) const SECRET_MARKER: &str = "qa-synthetic-credential-marker-not-real";

    pub(crate) fn form(fields: &[(&str, String)]) -> std::collections::HashMap<String, String> {
        fields
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    /// Returns the status and parsed problem body after asserting that no
    /// part of the response reflects `secret`.
    pub(crate) async fn secret_free_problem(
        response: Response,
        secret: &str,
    ) -> (u16, serde_json::Value) {
        let status = response.status().as_u16();
        for (name, value) in response.headers() {
            assert!(
                !value.as_bytes().windows(secret.len()).any(|w| w == secret.as_bytes()),
                "header {name} reflects the submitted credential"
            );
        }
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read OAuth error body: {error}"));
        let text = String::from_utf8(body.to_vec())
            .unwrap_or_else(|error| panic!("OAuth error body is not UTF-8: {error}"));
        assert!(!text.contains(secret), "{text}");
        let body = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("failed to parse OAuth error body: {error}"));
        (status, body)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{SECRET_MARKER, form, secret_free_problem};
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn should_reject_malformed_token_without_reflecting_it() {
        for token in [
            SECRET_MARKER.to_owned(),
            format!("aurahistoria_accesstoken_{SECRET_MARKER}"),
            String::from(RawOAuthClientSecret::new()),
        ] {
            let response = required_form_access_token(&form(&[("token", token.clone())]))
                .err()
                .unwrap_or_else(|| panic!("malformed token must be rejected"));
            let (status, body) = secret_free_problem(response, &token).await;

            assert_eq!(400, status);
            assert_eq!(
                json!({
                    "status": 400,
                    "title": "Bad Request",
                    "error": "BAD_BODY_VALUE",
                    "source": {"field": "token", "type": "BODY"},
                    "detail": "Form field 'token' is not a valid access token."
                }),
                body
            );
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_client_secret_without_reflecting_it() {
        for client_secret in [
            SECRET_MARKER.to_owned(),
            format!("aurahistoria_oauth_client_secret_{SECRET_MARKER}"),
            String::from(RawAccessToken::new()),
        ] {
            let response =
                required_form_client_secret(&form(&[("client_secret", client_secret.clone())]))
                    .err()
                    .unwrap_or_else(|| panic!("malformed client secret must be rejected"));
            let (status, body) = secret_free_problem(response, &client_secret).await;

            assert_eq!(401, status);
            assert_eq!(
                json!({
                    "status": 401,
                    "title": "Unauthorized",
                    "error": "INVALID_CREDENTIALS",
                    "source": {"field": "client_secret", "type": "BODY"},
                    "detail": "Form field 'client_secret' is not a valid OAuth client secret."
                }),
                body
            );
        }
    }

    #[test]
    fn should_accept_well_formed_credentials() {
        let token = RawAccessToken::new();
        let client_secret = RawOAuthClientSecret::new();
        let form = form(&[
            ("token", String::from(token.clone())),
            ("client_secret", String::from(client_secret.clone())),
        ]);

        assert!(matches!(required_form_access_token(&form), Ok(value) if value == token));
        assert!(
            matches!(required_form_client_secret(&form), Ok(value) if value == client_secret)
        );
    }

    #[test]
    fn should_parse_canonical_auction_read_for_oauth_scopes() {
        let expected = HashSet::from([Scope::AuctionsRead]);

        assert_eq!(
            expected,
            parse_scopes(["auctions:read".to_owned()]).unwrap_or_else(|_| unreachable!())
        );
        assert_eq!(
            expected,
            parse_scope_string(Some("auctions:read"), "scope").unwrap_or_else(|_| unreachable!())
        );
        assert_eq!("auctions:read", scope_string(&expected));
        assert_eq!(vec!["auctions:read".to_owned()], scope_strings(expected));
    }

    #[test]
    fn should_parse_canonical_product_listing_read_for_oauth_scopes() {
        let expected = HashSet::from([Scope::ProductListingsRead]);

        assert_eq!(
            expected,
            parse_scopes(["product-listings:read".to_owned()]).unwrap_or_else(|_| unreachable!())
        );
        assert_eq!(
            expected,
            parse_scope_string(Some("product-listings:read"), "scope")
                .unwrap_or_else(|_| unreachable!())
        );
        assert_eq!("product-listings:read", scope_string(&expected));
        assert_eq!(
            vec!["product-listings:read".to_owned()],
            scope_strings(expected)
        );
    }

    #[test]
    fn should_reject_noncanonical_auction_read_for_oauth_scopes() {
        for value in [
            "auction:read",
            "AUCTIONS_READ",
            "auctions:READ",
            "auctions:delete",
        ] {
            assert!(parse_scopes([value.to_owned()]).is_err(), "{value}");
            assert!(parse_scope_string(Some(value), "scope").is_err(), "{value}");
        }
    }

    #[test]
    fn should_parse_listing_source_write_for_oauth_scopes() {
        let expected = HashSet::from([Scope::ListingSourcesWrite]);

        assert_eq!(
            expected,
            parse_scopes(["listing-sources:write".to_owned()]).unwrap_or_else(|_| unreachable!())
        );
        assert_eq!(
            expected,
            parse_scope_string(Some("listing-sources:write"), "scope")
                .unwrap_or_else(|_| unreachable!())
        );
        assert_eq!("listing-sources:write", scope_string(&expected));
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
            let expected = HashSet::from([scope]);
            assert_eq!(
                expected,
                parse_scopes([value.to_owned()]).unwrap_or_else(|_| panic!("{value}"))
            );
            assert_eq!(
                expected,
                parse_scope_string(Some(value), "scope").unwrap_or_else(|_| panic!("{value}"))
            );
            assert_eq!(value, scope_string(&expected));
        }
    }
}
