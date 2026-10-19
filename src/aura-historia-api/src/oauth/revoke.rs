use super::{
    no_store, parse_form, required_form, required_form_access_token, required_form_client_secret,
};
use crate::error::ApiError;
use crate::state::OAuthState;
use crate::wire::parse_body_object_id;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::use_cases::RevokeTokenRequest;

pub async fn revoke(State(state): State<OAuthState>, body: String) -> Response {
    let form = parse_form(body);
    let request = match request(&form) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.revoke.execute(&system_context(), request).await {
        Ok(()) => no_store(axum::http::StatusCode::OK.into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn request(
    form: &std::collections::HashMap<String, String>,
) -> Result<RevokeTokenRequest, Response> {
    let token = required_form_access_token(form)?;
    let client_id: OAuthClientId = parse_body_object_id(
        required_form(form, "client_id")?,
        "client_id",
        "OAuthClient",
    )
    .map_err(IntoResponse::into_response)?;
    let client_secret = required_form_client_secret(form)?;
    Ok(RevokeTokenRequest {
        token,
        client_id,
        client_secret,
    })
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
            assert_eq!("BAD_BODY_VALUE", body["error"]);
            assert_eq!(json!({"field": "token", "type": "BODY"}), body["source"]);
        }
    }

    #[tokio::test]
    async fn should_reject_malformed_client_secret_without_reflecting_it() {
        for client_secret in [SECRET_MARKER.to_owned(), String::from(RawAccessToken::new())] {
            let response = request(&form(&[
                ("token", String::from(RawAccessToken::new())),
                ("client_id", OAuthClientId::new().to_string()),
                ("client_secret", client_secret.clone()),
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
