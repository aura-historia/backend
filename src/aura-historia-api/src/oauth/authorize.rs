use super::error::OAuthError;
use super::{OAuthQuery, no_store, parse_requested_scope};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::OAuthState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use oauth_core::authorization_code::{CodeChallengeMethod, OAuthCodeChallenge};
use oauth_core::client_identifier::OAuthClientIdentifier;
use oauth_service::use_cases::{
    AuthorizeRequest, OAuthResponseType, OAuthState as OAuthRequestState,
};
use std::collections::HashMap;

pub(crate) async fn authorize(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    OAuthQuery(query): OAuthQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match request(query) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    match state.authorize.execute(&context, request).await {
        Ok(result) => match result.redirect_to.parse::<axum::http::HeaderValue>() {
            Ok(location) => {
                no_store((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
            }
            Err(source) => OAuthError::ServerError {
                source: application::error::box_error(source),
            }
            .into_response(),
        },
        Err(error) => match error {
            oauth_service::error::OAuthServiceError::ClientNotFound => {
                OAuthError::InvalidRequest.into_response()
            }
            // The approving browser is authenticated; 401 would read as its own sign-in failing.
            oauth_service::error::OAuthServiceError::InvalidMetadataDocumentClient { .. } => {
                OAuthError::InvalidClient.into_response_with_status(StatusCode::BAD_REQUEST)
            }
            error @ (oauth_service::error::OAuthServiceError::AuthenticatedActorRequired
            | oauth_service::error::OAuthServiceError::Forbidden
            | oauth_service::error::OAuthServiceError::DelegatedAuthorizationForbidden) => {
                no_store(ApiError::from(error).into_response())
            }
            error => OAuthError::from(error).into_response(),
        },
    }
}

fn request(query: HashMap<String, String>) -> Result<AuthorizeRequest, OAuthError> {
    let response_type = match required(&query, "response_type")? {
        "code" => OAuthResponseType::Code,
        _ => return Err(OAuthError::UnsupportedResponseType),
    };
    let client_id = OAuthClientIdentifier::parse(required(&query, "client_id")?)
        .map_err(|_| OAuthError::InvalidRequest)?;
    let redirect_uri = url::Url::parse(required(&query, "redirect_uri")?)
        .map_err(|_| OAuthError::InvalidRequest)?;
    let code_challenge_method = match required(&query, "code_challenge_method")? {
        "S256" => CodeChallengeMethod::S256,
        _ => return Err(OAuthError::InvalidRequest),
    };
    let challenge = required(&query, "code_challenge")?;
    if challenge.len() != 43
        || !challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(OAuthError::InvalidRequest);
    }
    let scope = parse_requested_scope(query.get("scope").map(String::as_str))
        .map_err(|_| OAuthError::InvalidScope)?;
    Ok(AuthorizeRequest {
        response_type,
        client_id,
        redirect_uri,
        scope: scope.scopes,
        offline_access: scope.offline_access,
        state: query
            .get("state")
            .map(|value| OAuthRequestState::from(value.as_str())),
        code_challenge: OAuthCodeChallenge::from(challenge),
        code_challenge_method,
        resource: query.get("resource").cloned(),
    })
}

fn required<'a>(
    query: &'a HashMap<String, String>,
    field: &'static str,
) -> Result<&'a str, OAuthError> {
    super::required_form(query, field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
    use credential_core::oauth_client_id::OAuthClientId;

    #[test]
    fn should_forward_the_exact_optional_resource_to_the_service() {
        let mut query = HashMap::from([
            ("response_type".to_owned(), "code".to_owned()),
            ("client_id".to_owned(), OAuthClientId::new().to_string()),
            (
                "redirect_uri".to_owned(),
                "https://client.example/callback".to_owned(),
            ),
            (
                "code_challenge".to_owned(),
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
            ),
            ("code_challenge_method".to_owned(), "S256".to_owned()),
        ]);
        assert_eq!(None, request(query.clone()).unwrap().resource);
        for resource in [
            "https://mcp.aura-historia.com/v1",
            "https://foreign.example#fragment",
            "",
        ] {
            query.insert("resource".to_owned(), resource.to_owned());
            assert_eq!(
                Some(resource.to_owned()),
                request(query.clone()).unwrap().resource
            );
        }
    }
    #[test]
    fn should_forward_either_client_kind_and_reject_unparsable_client_ids() {
        let mut query = HashMap::from([
            ("response_type".to_owned(), "code".to_owned()),
            (
                "redirect_uri".to_owned(),
                "http://127.0.0.1:4000/callback".to_owned(),
            ),
            (
                "code_challenge".to_owned(),
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
            ),
            ("code_challenge_method".to_owned(), "S256".to_owned()),
        ]);
        let registered = OAuthClientId::new();
        for client_id in [
            registered.to_string(),
            "https://agent.example/client.json".to_owned(),
        ] {
            query.insert("client_id".to_owned(), client_id.clone());
            assert_eq!(
                client_id,
                request(query.clone()).unwrap().client_id.to_string()
            );
        }
        for client_id in [
            "https://agent.example",
            "https://agent.example:8443/c",
            "oc_x",
        ] {
            query.insert("client_id".to_owned(), client_id.to_owned());
            assert!(matches!(
                request(query.clone()),
                Err(OAuthError::InvalidRequest)
            ));
        }
    }

    #[tokio::test]
    async fn should_report_invalid_scope_and_unsupported_response_without_echoing_input() {
        let valid = form(&[
            ("response_type", "code".to_owned()),
            ("client_id", OAuthClientId::new().to_string()),
            ("redirect_uri", "https://client.example/callback".to_owned()),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
            ),
            ("code_challenge_method", "S256".to_owned()),
        ]);
        for (field, code) in [
            ("scope", "invalid_scope"),
            ("response_type", "unsupported_response_type"),
            ("code_challenge_method", "invalid_request"),
            ("code_challenge", "invalid_request"),
        ] {
            let mut query = valid.clone();
            query.insert(field.to_owned(), SECRET_MARKER.to_owned());
            let error = request(query).unwrap_err();
            let (status, body) = secret_free_problem(error, SECRET_MARKER).await;
            assert_eq!(400, status);
            assert_eq!(serde_json::json!({"error": code}), body);
        }
    }
}
