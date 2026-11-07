use super::error::OAuthError;
use super::token::client_authentication;
use super::{OAuthForm, no_store, required_form};
use crate::state::OAuthState;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use oauth_core::refresh_token::RawRefreshToken;
use oauth_service::use_cases::{RevocableToken, RevokeTokenRequest};
use user_core::access_token::RawAccessToken;

pub(crate) async fn revoke(
    State(state): State<OAuthState>,
    OAuthForm(form): OAuthForm,
) -> Response {
    let request = match request(&form) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state.revoke.execute(&system_context(), request).await {
        Ok(()) => no_store(axum::http::StatusCode::OK.into_response()),
        Err(error) => OAuthError::from(error).into_response(),
    }
}

/// A registered client authenticates with its secret; a metadata document client with its
/// `client_id` alone. Either may name an access token; only the latter holds refresh tokens.
fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<RevokeTokenRequest, OAuthError> {
    let token = revocable_token(required_form(form, "token")?)?;
    Ok(RevokeTokenRequest {
        token,
        client: client_authentication(form)?,
    })
}

fn revocable_token(value: &str) -> Result<RevocableToken, OAuthError> {
    RawAccessToken::try_from(value.to_owned())
        .map(RevocableToken::AccessToken)
        .or_else(|_| RawRefreshToken::try_from(value.to_owned()).map(RevocableToken::RefreshToken))
        .map_err(|_| OAuthError::InvalidRequest)
}

fn system_context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("oauth-revoke"),
        correlation_id: CorrelationId::new("oauth-revoke"),
    }
}

#[cfg(test)]
mod tests {
    use super::request;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
    use credential_core::oauth_client_id::OAuthClientId;
    use serde_json::json;
    use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

    #[tokio::test]
    async fn should_reject_malformed_token_without_reflecting_it() {
        for token in [
            SECRET_MARKER.to_owned(),
            String::from(RawOAuthClientSecret::new()),
        ] {
            let response = request(&form(&[
                ("token", token.clone()),
                ("client_id", OAuthClientId::new().to_string()),
                ("client_secret", String::from(RawOAuthClientSecret::new())),
            ]))
            .err()
            .unwrap_or_else(|| panic!("malformed token must be rejected"));
            let (status, body) = secret_free_problem(response, &token).await;

            assert_eq!(400, status);
            assert_eq!("invalid_request", body["error"]);
            assert_eq!(json!({"error": "invalid_request"}), body);
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_client_secret_without_reflecting_it() {
        for client_secret in [
            SECRET_MARKER.to_owned(),
            String::from(RawAccessToken::new()),
        ] {
            let response = request(&form(&[
                ("token", String::from(RawAccessToken::new())),
                ("client_id", OAuthClientId::new().to_string()),
                ("client_secret", client_secret.clone()),
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
    fn should_accept_access_and_refresh_tokens_from_metadata_document_clients() {
        use oauth_core::refresh_token::RawRefreshToken;
        use oauth_service::use_cases::{OAuthTokenClientAuthentication, RevocableToken};
        let refresh_token = RawRefreshToken::new();
        let access_token = RawAccessToken::new();
        for (token, expected) in [
            (
                String::from(refresh_token.clone()),
                RevocableToken::RefreshToken(refresh_token),
            ),
            (
                String::from(access_token.clone()),
                RevocableToken::AccessToken(access_token),
            ),
        ] {
            let request = request(&form(&[
                ("token", token),
                ("client_id", "https://agent.example/client.json".to_owned()),
            ]))
            .unwrap_or_else(|_| panic!("a public client revokes without a secret"));
            assert_eq!(expected, request.token);
            assert_eq!(
                OAuthTokenClientAuthentication::None {
                    client_id: "https://agent.example/client.json".parse().unwrap()
                },
                request.client
            );
        }
    }
}
