use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_CREDENTIALS};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

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
    RawOAuthClientSecret::try_from(required_form(form, "client_secret")?.to_owned()).map_err(|_| {
        ApiError::unauthorized(INVALID_CREDENTIALS)
            .with_body_field("client_secret")
            .with_detail("Form field 'client_secret' is not a valid OAuth client secret.")
            .into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::test_support::{SECRET_MARKER, form, secret_free_problem};
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
        assert!(matches!(required_form_client_secret(&form), Ok(value) if value == client_secret));
    }
}
