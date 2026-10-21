use super::error::OAuthError;
use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    http::{header, request::Parts},
};
use credential_core::oauth_client_id::OAuthClientId;
use std::collections::HashMap;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

pub(crate) struct OAuthForm(pub HashMap<String, String>);

impl<S: Send + Sync> FromRequest<S> for OAuthForm {
    type Rejection = OAuthError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        // These endpoints accept client_secret_post exclusively; a header would
        // introduce a second or unsupported authentication mechanism.
        if request.headers().contains_key(header::AUTHORIZATION) {
            return Err(OAuthError::InvalidRequest);
        }
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next());
        if !content_type.is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        }) {
            return Err(OAuthError::InvalidRequest);
        }
        let body = String::from_request(request, state)
            .await
            .map_err(|_| OAuthError::InvalidRequest)?;
        Ok(Self(parse_parameters(&body)?))
    }
}

pub(crate) struct OAuthQuery(pub HashMap<String, String>);

impl<S: Send + Sync> FromRequestParts<S> for OAuthQuery {
    type Rejection = OAuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(parse_parameters(
            parts.uri.query().unwrap_or_default(),
        )?))
    }
}

/// Preserve all parameters until duplicates have been rejected, including encoded duplicate keys.
fn parse_parameters(raw: &str) -> Result<HashMap<String, String>, OAuthError> {
    let mut parameters = HashMap::new();
    for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key)?;
        let value = decode_component(value)?;
        if key.is_empty() {
            return Err(OAuthError::InvalidRequest);
        }
        if parameters.contains_key(&key) {
            return Err(if key == "resource" {
                OAuthError::InvalidTarget
            } else {
                OAuthError::InvalidRequest
            });
        }
        parameters.insert(key, value);
    }
    Ok(parameters)
}

fn decode_component(raw: &str) -> Result<String, OAuthError> {
    let mut decoded = Vec::with_capacity(raw.len());
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(match byte {
            b'+' => b' ',
            b'%' => {
                let high = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
                let low = bytes.next().and_then(|byte| char::from(byte).to_digit(16));
                match (high, low) {
                    (Some(high), Some(low)) => (high * 16 + low) as u8,
                    _ => return Err(OAuthError::InvalidRequest),
                }
            }
            byte => byte,
        });
    }
    String::from_utf8(decoded).map_err(|_| OAuthError::InvalidRequest)
}

pub(crate) fn required_form<'a>(
    form: &'a HashMap<String, String>,
    field: &'static str,
) -> Result<&'a str, OAuthError> {
    form.get(field)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(OAuthError::InvalidRequest)
}

pub(crate) fn required_form_client_id(
    form: &HashMap<String, String>,
) -> Result<OAuthClientId, OAuthError> {
    required_form(form, "client_id")?
        .parse()
        .map_err(|_| OAuthError::InvalidClient)
}

pub(crate) fn required_form_access_token(
    form: &HashMap<String, String>,
) -> Result<RawAccessToken, OAuthError> {
    RawAccessToken::try_from(required_form(form, "token")?.to_owned())
        .map_err(|_| OAuthError::InvalidRequest)
}

pub(crate) fn required_form_client_secret(
    form: &HashMap<String, String>,
) -> Result<RawOAuthClientSecret, OAuthError> {
    RawOAuthClientSecret::try_from(required_form(form, "client_secret")?.to_owned())
        .map_err(|_| OAuthError::InvalidClient)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
    use axum::{
        Router,
        body::Body,
        http::Request,
        routing::{get, post},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn should_reject_malformed_credentials_without_reflecting_them() {
        for token in [
            SECRET_MARKER.to_owned(),
            String::from(RawOAuthClientSecret::new()),
        ] {
            let error = required_form_access_token(&form(&[("token", token.clone())])).unwrap_err();
            let (status, body) = secret_free_problem(error, &token).await;
            assert_eq!(400, status);
            assert_eq!(serde_json::json!({"error": "invalid_request"}), body);
        }
        for secret in [
            SECRET_MARKER.to_owned(),
            String::from(RawAccessToken::new()),
        ] {
            let error = required_form_client_secret(&form(&[("client_secret", secret.clone())]))
                .unwrap_err();
            let (status, body) = secret_free_problem(error, &secret).await;
            assert_eq!(401, status);
            assert_eq!(serde_json::json!({"error": "invalid_client"}), body);
        }
    }

    #[test]
    fn should_accept_well_formed_credentials_and_normal_form_encoding() {
        let token = RawAccessToken::new();
        let secret = RawOAuthClientSecret::new();
        let form = form(&[
            ("token", String::from(token.clone())),
            ("client_secret", String::from(secret.clone())),
        ]);
        assert_eq!(token, required_form_access_token(&form).unwrap());
        assert_eq!(secret, required_form_client_secret(&form).unwrap());
        let parameters = parse_parameters("redirect_uri=https%3A%2F%2Fclient.example%2Fcallback%3Fa%3Db%26c%3Dd&scope=users%3Aread+watchlist%3Aread").unwrap();
        assert_eq!(
            "https://client.example/callback?a=b&c=d",
            parameters["redirect_uri"]
        );
        assert_eq!("users:read watchlist:read", parameters["scope"]);
    }

    #[test]
    fn should_reject_duplicate_parameters_and_invalid_encoding() {
        for raw in [
            "client_id=a&client_id=b",
            "scope=a&scope=b",
            "code=%",
            "code=%2",
            "code=%GG",
            "code=%FF",
            "=value",
        ] {
            assert!(
                matches!(parse_parameters(raw), Err(OAuthError::InvalidRequest)),
                "{raw}"
            );
        }
    }

    #[test]
    fn should_reject_multiple_resource_indicators_without_collapsing_them() {
        for raw in [
            "resource=foreign&resource=allowed",
            "resource=allowed&%72esource=foreign",
            "resource=&resource=allowed",
        ] {
            assert!(matches!(
                parse_parameters(raw),
                Err(OAuthError::InvalidTarget)
            ));
        }
    }

    #[tokio::test]
    async fn should_apply_oauth_errors_to_form_and_query_extractor_failures() {
        let app = Router::new()
            .route(
                "/token",
                post(|OAuthForm(_): OAuthForm| async { "accepted" }),
            )
            .route(
                "/authorize",
                get(|OAuthQuery(_): OAuthQuery| async { "accepted" }),
            );
        for (path, content_type, body, authorization, expected_status, expected_error) in [
            (
                "/token",
                "application/json",
                "{}",
                None,
                400,
                "invalid_request",
            ),
            (
                "/token",
                "application/x-www-form-urlencoded",
                "client_id=a&client_id=b",
                None,
                400,
                "invalid_request",
            ),
            (
                "/token",
                "application/x-www-form-urlencoded",
                "client_id=a",
                Some("Basic synthetic"),
                400,
                "invalid_request",
            ),
            (
                "/authorize?resource=allowed&resource=foreign",
                "",
                "",
                None,
                400,
                "invalid_target",
            ),
        ] {
            let mut request = Request::builder()
                .uri(path)
                .method(if path.starts_with("/token") {
                    "POST"
                } else {
                    "GET"
                })
                .header(header::CONTENT_TYPE, content_type);
            if let Some(value) = authorization {
                request = request.header(header::AUTHORIZATION, value);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            let (status, body) = secret_free_problem(response, SECRET_MARKER).await;
            assert_eq!(expected_status, status);
            assert_eq!(serde_json::json!({"error": expected_error}), body);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/token")
                    .method("POST")
                    .header(
                        header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded; charset=UTF-8",
                    )
                    .body(Body::from("scope=users%3Aread+watchlist%3Aread"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(200, response.status());
    }
}
