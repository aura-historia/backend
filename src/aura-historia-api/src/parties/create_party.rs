use super::types::PartyData;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, PARTY_INTERNAL_ERROR};
use crate::state::PartiesState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use party_core::party::PartyContact;
use party_core::party_name::PartyName;
use party_service::use_cases::commands::create_party::CreatePartyCommand;
use serde::Deserialize;
use serde_email::Email;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatePartyData {
    name: String,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    email: Option<Email>,
}

pub async fn create_party(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let command = match parse_body(&body) {
        Ok(command) => command,
        Err(error) => return error.into_response(),
    };

    match state.create_party.execute(&context, command).await {
        Ok(result) => {
            let party_id = result.party_id;
            let mut response = (StatusCode::CREATED, Json(PartyData::from(result))).into_response();
            let location = format!("/api/v1/admin/parties/{party_id}");
            let location = match HeaderValue::from_str(&location) {
                Ok(value) => value,
                Err(_) => {
                    return ApiError::internal_server_error(PARTY_INTERNAL_ERROR)
                        .with_detail("Party location failed internally.")
                        .into_response();
                }
            };
            response.headers_mut().insert(header::LOCATION, location);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_body(body: &str) -> Result<CreatePartyCommand, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }

    let data: CreatePartyData = serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    let name = PartyName::try_from(data.name).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("name must be nonblank and at most 255 UTF-8 bytes.")
    })?;

    Ok(CreatePartyCommand {
        name,
        contact: PartyContact {
            phone: data.phone,
            email: data.email,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{CONFLICT, PARTY_TEMPORARILY_UNAVAILABLE};
    use application::error::static_error;
    use axum::response::IntoResponse;
    use party_service::use_cases::commands::create_party::CreatePartyError;

    #[test]
    fn should_map_party_name_and_all_contact_fields_to_command() -> Result<(), ApiError> {
        let command = parse_body(
            r#"{
                "name": "  Antiques and More  ",
                "phone": "+49 30 123456",
                "email": "contact@example.com"
            }"#,
        )?;

        assert_eq!("Antiques and More", command.name.as_ref());
        assert_eq!(Some("+49 30 123456"), command.contact.phone.as_deref());
        assert_eq!(
            Some("contact@example.com"),
            command
                .contact
                .email
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
        );
        Ok(())
    }

    #[test]
    fn should_allow_each_optional_contact_variant() -> Result<(), ApiError> {
        for body in [
            r#"{"name":"No contact"}"#,
            r#"{"name":"Phone only","phone":"+49 30 123456"}"#,
            r#"{"name":"Email only","email":"contact@example.com"}"#,
        ] {
            parse_body(body)?;
        }
        Ok(())
    }

    #[test]
    fn should_reject_invalid_party_names_at_the_api_boundary() {
        for name in ["", " \u{2003}\u{00a0}", &"é".repeat(128)] {
            let body = serde_json::json!({"name": name}).to_string();
            let error = parse_body(&body).err();
            assert!(matches!(
                error.map(|value| value.code()),
                Some(BAD_BODY_VALUE)
            ));
        }
    }

    #[test]
    fn should_map_create_party_persistence_errors_to_canonical_problems() {
        let conflict = ApiError::from(CreatePartyError::SlugConflict {
            source: static_error("party slug already exists"),
        });
        assert_eq!(CONFLICT, conflict.code());
        assert_eq!(StatusCode::CONFLICT, conflict.into_response().status());

        let temporary = ApiError::from(CreatePartyError::TemporarilyUnavailable {
            source: static_error("database unavailable"),
        });
        assert_eq!(PARTY_TEMPORARILY_UNAVAILABLE, temporary.code());
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            temporary.into_response().status()
        );
    }
}
