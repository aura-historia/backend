use super::error::OAuthError;
use super::{
    OAuthForm, no_store, parse_requested_scope, required_form, required_form_client_secret,
    scope_string,
};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use oauth_core::authorization_code::{OAuthAuthorizationCode, OAuthCodeVerifier};

use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_core::refresh_token::RawRefreshToken;
use oauth_service::use_cases::{
    OAuthGrantType, OAuthTokenClientAuthentication, OAuthTokenType,
    TokenByAuthorizationCodeRequest, TokenByRefreshTokenRequest, TokenResponse,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
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
            refresh_token: response.refresh_token.map(String::from),
        }
    }
}

pub(crate) async fn token(State(state): State<OAuthState>, OAuthForm(form): OAuthForm) -> Response {
    let result = match grant_type(&form) {
        Ok(OAuthGrantType::AuthorizationCode) => match request(&form) {
            Ok(request) => state.token_by_authorization_code.execute(request).await,
            Err(error) => return error.into_response(),
        },
        Ok(OAuthGrantType::RefreshToken) => match refresh_request(&form) {
            Ok(request) => state.token_by_refresh_token.execute(request).await,
            Err(error) => return error.into_response(),
        },
        Err(error) => return error.into_response(),
    };
    match result {
        Ok(result) => no_store(Json(TokenResponseData::from(result)).into_response()),
        Err(error) => OAuthError::from(error).into_response(),
    }
}

fn grant_type(
    form: &std::collections::HashMap<String, String>,
) -> Result<OAuthGrantType, OAuthError> {
    match required_form(form, "grant_type")? {
        "authorization_code" => Ok(OAuthGrantType::AuthorizationCode),
        "refresh_token" => Ok(OAuthGrantType::RefreshToken),
        _ => Err(OAuthError::UnsupportedGrantType),
    }
}

/// Only metadata document clients hold refresh tokens. They authenticate with `client_id`
/// alone; registered clients are not allowed this grant.
fn refresh_request(
    form: &std::collections::HashMap<String, String>,
) -> Result<TokenByRefreshTokenRequest, OAuthError> {
    if matches!(
        OAuthClientIdentifier::parse(required_form(form, "client_id")?),
        Ok(OAuthClientIdentifier::Registered(_))
    ) {
        return Err(OAuthError::UnauthorizedClient);
    }
    let OAuthTokenClientAuthentication::None { client_id } = client_authentication(form)? else {
        return Err(OAuthError::UnauthorizedClient);
    };
    let refresh_token = RawRefreshToken::try_from(required_form(form, "refresh_token")?.to_owned())
        .map_err(|_| OAuthError::InvalidGrant)?;
    let scope = form
        .get("scope")
        .map(|scope| {
            parse_requested_scope(Some(scope))
                .map(|requested| requested.scopes)
                .map_err(|_| OAuthError::InvalidScope)
        })
        .transpose()?;
    Ok(TokenByRefreshTokenRequest {
        refresh_token,
        client_id,
        scope,
        resource: form.get("resource").cloned(),
    })
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<TokenByAuthorizationCodeRequest, OAuthError> {
    let grant_type = match grant_type(form)? {
        grant_type @ OAuthGrantType::AuthorizationCode => grant_type,
        OAuthGrantType::RefreshToken => return Err(OAuthError::UnsupportedGrantType),
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
        client: client_authentication(form)?,
        code_verifier: OAuthCodeVerifier::from(verifier),
        resource: form.get("resource").cloned(),
    })
}

/// A registered client authenticates with `client_secret_post`; a metadata document client is
/// public and presents its `client_id` alone, so a secret from it is a client error.
pub(crate) fn client_authentication(
    form: &std::collections::HashMap<String, String>,
) -> Result<OAuthTokenClientAuthentication, OAuthError> {
    match OAuthClientIdentifier::parse(required_form(form, "client_id")?) {
        Ok(OAuthClientIdentifier::Registered(client_id)) => {
            Ok(OAuthTokenClientAuthentication::ClientSecretPost {
                client_id,
                client_secret: required_form_client_secret(form)?,
            })
        }
        Ok(OAuthClientIdentifier::MetadataDocument(client_id)) => {
            if form.contains_key("client_secret") {
                return Err(OAuthError::InvalidClient);
            }
            Ok(OAuthTokenClientAuthentication::None { client_id })
        }
        Err(_) => Err(OAuthError::InvalidClient),
    }
}

#[cfg(test)]
mod tests {
    use super::request;
    use crate::oauth::error::OAuthError;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
    use credential_core::oauth_client_id::OAuthClientId;
    use oauth_core::authorization_code::OAuthAuthorizationCode;
    use oauth_service::use_cases::OAuthTokenClientAuthentication;
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

    #[test]
    fn should_authenticate_each_client_kind_with_its_own_method() {
        let base = form(&[
            ("grant_type", "authorization_code".to_owned()),
            ("code", OAuthAuthorizationCode::new().to_string()),
            ("redirect_uri", "http://127.0.0.1:4000/callback".to_owned()),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned(),
            ),
        ]);
        let mut public = base.clone();
        public.insert(
            "client_id".to_owned(),
            "https://agent.example/client.json".to_owned(),
        );
        assert_eq!(
            OAuthTokenClientAuthentication::None {
                client_id: "https://agent.example/client.json".parse().unwrap()
            },
            request(&public).unwrap().client
        );

        let mut with_secret = public.clone();
        with_secret.insert(
            "client_secret".to_owned(),
            String::from(user_core::access_token::RawOAuthClientSecret::new()),
        );
        let mut registered_without_secret = base.clone();
        registered_without_secret.insert("client_id".to_owned(), OAuthClientId::new().to_string());
        let mut noncanonical = base.clone();
        noncanonical.insert(
            "client_id".to_owned(),
            "https://agent.example:443/client.json".to_owned(),
        );
        for form in [with_secret, noncanonical] {
            assert!(matches!(request(&form), Err(OAuthError::InvalidClient)));
        }
        assert!(matches!(
            request(&registered_without_secret),
            Err(OAuthError::InvalidRequest)
        ));
        assert!(matches!(request(&base), Err(OAuthError::InvalidRequest)));
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

    #[test]
    fn should_accept_refresh_grants_only_from_metadata_document_clients() {
        use super::refresh_request;
        let refresh_token = String::from(oauth_core::refresh_token::RawRefreshToken::new());
        let public = form(&[
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh_token.clone()),
            ("client_id", "https://agent.example/client.json".to_owned()),
            ("scope", "users:read offline_access".to_owned()),
        ]);
        let request = refresh_request(&public).unwrap();
        assert_eq!(
            "https://agent.example/client.json",
            request.client_id.as_str()
        );
        assert_eq!(
            Some(std::collections::HashSet::from([
                user_core::access_token::Scope::UsersRead
            ])),
            request.scope
        );

        let mut registered = public.clone();
        registered.insert("client_id".to_owned(), OAuthClientId::new().to_string());
        assert!(matches!(
            refresh_request(&registered),
            Err(OAuthError::UnauthorizedClient)
        ));
        let mut with_secret = public.clone();
        with_secret.insert(
            "client_secret".to_owned(),
            String::from(user_core::access_token::RawOAuthClientSecret::new()),
        );
        assert!(matches!(
            refresh_request(&with_secret),
            Err(OAuthError::InvalidClient)
        ));
        for token in [
            SECRET_MARKER.to_owned(),
            String::from(RawAccessToken::new()),
        ] {
            let mut malformed = public.clone();
            malformed.insert("refresh_token".to_owned(), token);
            assert!(matches!(
                refresh_request(&malformed),
                Err(OAuthError::InvalidGrant)
            ));
        }
        let mut unknown_scope = public;
        unknown_scope.insert("scope".to_owned(), "openid".to_owned());
        assert!(matches!(
            refresh_request(&unknown_scope),
            Err(OAuthError::InvalidScope)
        ));
    }
}
