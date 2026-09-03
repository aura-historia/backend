use super::types::PartyData;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::PartiesState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use party_core::party_id::PartyId;
use party_service::use_cases::queries::get_party::GetPartyRequest;
use uuid::Uuid;

pub async fn get_party(
    State(state): State<PartiesState>,
    headers: HeaderMap,
    Path(raw_party_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let party_id = match parse_party_id(&raw_party_id) {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state
        .get_party
        .execute(&context, GetPartyRequest::ById(party_id))
        .await
    {
        Ok(result) => no_store(Json(PartyData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_party_id(raw: &str) -> Result<PartyId, ApiError> {
    Uuid::parse_str(raw).map(PartyId::from).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("partyId")
            .with_detail("Path parameter 'partyId' must be a UUID.")
    })
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn should_parse_party_id_from_uuid_path_value() {
        let party_id = parse_party_id("550e8400-e29b-41d4-a716-446655440000")
            .unwrap_or_else(|error| panic!("failed to parse party ID: {error}"));

        assert_eq!("550e8400-e29b-41d4-a716-446655440000", party_id.to_string());
    }

    #[test]
    fn should_report_invalid_party_id_as_path_uuid_problem() {
        let error = match parse_party_id("not-a-uuid") {
            Ok(_) => panic!("invalid party ID was accepted"),
            Err(error) => error,
        };

        assert_eq!(INVALID_UUID, error.code());
        let response = error.into_response();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
    }
}
