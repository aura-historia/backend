use super::{no_store, parse_form, required_form, required_form_client_secret, scope_string};
use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::state::OAuthState;
use crate::wire::parse_body_object_id;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
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

pub async fn token(State(state): State<OAuthState>, body: String) -> Response {
    let form = parse_form(body);
    let request = match request(&form) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.token_by_authorization_code.execute(request).await {
        Ok(result) => no_store(Json(TokenResponseData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<TokenByAuthorizationCodeRequest, Response> {
    let grant_type = match required_form(form, "grant_type")? {
        "authorization_code" => OAuthGrantType::AuthorizationCode,
        value => {
            return Err(ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(format!("Unsupported grant_type '{value}'."))
                .into_response());
        }
    };
    let code = OAuthAuthorizationCode::try_from(required_form(form, "code")?.to_owned()).map_err(
        |error| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(error.to_string())
                .into_response()
        },
    )?;
    let redirect_uri = url::Url::parse(required_form(form, "redirect_uri")?).map_err(|error| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(error.to_string())
            .into_response()
    })?;
    let client_id: OAuthClientId = parse_body_object_id(
        required_form(form, "client_id")?,
        "client_id",
        "OAuthClient",
    )
    .map_err(IntoResponse::into_response)?;
    let client_secret = required_form_client_secret(form)?;
    Ok(TokenByAuthorizationCodeRequest {
        grant_type,
        code,
        redirect_uri,
        client_id,
        client_secret,
        code_verifier: OAuthCodeVerifier::from(required_form(form, "code_verifier")?),
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
                ("code_verifier", "code-verifier".to_owned()),
            ]))
            .err()
            .unwrap_or_else(|| panic!("malformed client secret must be rejected"));
            let (status, body) = secret_free_problem(response, &client_secret).await;

            assert_eq!(401, status);
            assert_eq!("INVALID_CREDENTIALS", body["error"]);
            assert_eq!(
                json!({"field": "client_secret", "type": "BODY"}),
                body["source"]
            );
        }
    }
}
