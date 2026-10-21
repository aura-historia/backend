use super::error::OAuthError;
use super::{
    OAuthForm, no_store, required_form, required_form_client_id, required_form_client_secret,
    scope_string,
};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use oauth_core::authorization_code::{OAuthAuthorizationCode, OAuthCodeVerifier};

use oauth_service::use_cases::{
    OAuthGrantType, OAuthTokenType, TokenByAuthorizationCodeRequest, TokenResponse,
};
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Serialize)]
pub(crate) struct TokenResponseData {
    access_token: String,
    token_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<i64>,
    scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    third_party_exchange_code: Option<String>,
}

impl From<TokenResponse> for TokenResponseData {
    fn from(response: TokenResponse) -> Self {
        let token_type = match response.token_type {
            OAuthTokenType::Bearer => "Bearer",
        };
        Self {
            access_token: response.access_token.into(),
            token_type,
            expires_in: response
                .expires
                .map(|expires| (expires - OffsetDateTime::now_utc()).whole_seconds().max(0)),
            scope: scope_string(&response.scopes),
            third_party_exchange_code: response
                .third_party_exchange_code
                .map(|code| code.to_string()),
        }
    }
}

pub(crate) async fn token(State(state): State<OAuthState>, OAuthForm(form): OAuthForm) -> Response {
    let request = match request(&form) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state.token_by_authorization_code.execute(request).await {
        Ok(result) => no_store(Json(TokenResponseData::from(result)).into_response()),
        Err(error) => OAuthError::from(error).into_response(),
    }
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<TokenByAuthorizationCodeRequest, OAuthError> {
    let grant_type = match required_form(form, "grant_type")? {
        "authorization_code" => OAuthGrantType::AuthorizationCode,
        _ => return Err(OAuthError::UnsupportedGrantType),
    };
    let code = OAuthAuthorizationCode::try_from(required_form(form, "code")?.to_owned())
        .map_err(|_| OAuthError::InvalidGrant)?;
    let redirect_uri = url::Url::parse(required_form(form, "redirect_uri")?)
        .map_err(|_| OAuthError::InvalidRequest)?;
    let verifier = required_form(form, "code_verifier")?;
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"-._~".contains(&value))
    {
        return Err(OAuthError::InvalidGrant);
    }
    Ok(TokenByAuthorizationCodeRequest {
        grant_type,
        code,
        redirect_uri,
        client_id: required_form_client_id(form)?,
        client_secret: required_form_client_secret(form)?,
        code_verifier: OAuthCodeVerifier::from(verifier),
        resource: form.get("resource").cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::request;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
    use credential_core::oauth_client_id::OAuthClientId;
    use oauth_core::authorization_code::OAuthAuthorizationCode;
    use serde_json::json;
    use user_core::access_token::RawAccessToken;

    #[test]
    fn should_forward_the_exact_optional_resource_to_the_service() {
        let mut values = form(&[
            ("grant_type", "authorization_code".to_owned()),
            ("code", OAuthAuthorizationCode::new().to_string()),
            ("redirect_uri", "https://client.example/callback".to_owned()),
            ("client_id", OAuthClientId::new().to_string()),
            (
                "client_secret",
                String::from(user_core::access_token::RawOAuthClientSecret::new()),
            ),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned(),
            ),
        ]);
        assert_eq!(None, request(&values).unwrap().resource);
        for resource in [
            "https://mcp.aura-historia.com/v1",
            "https://foreign.example#fragment",
            "",
        ] {
            values.insert("resource".to_owned(), resource.to_owned());
            assert_eq!(
                Some(resource.to_owned()),
                request(&values).unwrap().resource
            );
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_client_secret_without_reflecting_it() {
        for client_secret in [
            SECRET_MARKER.to_owned(),
            String::from(RawAccessToken::new()),
        ] {
            let response = request(&form(&[
                ("grant_type", "authorization_code".to_owned()),
                ("code", OAuthAuthorizationCode::new().to_string()),
                ("redirect_uri", "https://client.example/callback".to_owned()),
                ("client_id", OAuthClientId::new().to_string()),
                ("client_secret", client_secret.clone()),
                (
                    "code_verifier",
                    "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned(),
                ),
            ]))
            .err()
            .unwrap_or_else(|| panic!("malformed client secret must be rejected"));
            let (status, body) = secret_free_problem(response, &client_secret).await;

            assert_eq!(401, status);
            assert_eq!("invalid_client", body["error"]);
            assert_eq!(json!({"error": "invalid_client"}), body);
        }
    }
}
