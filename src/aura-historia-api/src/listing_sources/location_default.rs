use crate::{
    auth::protected_context,
    error::{
        ApiError, BAD_BODY_VALUE, CONFLICT, FORBIDDEN, INVALID_CREDENTIALS,
        LISTING_SOURCE_INTERNAL_ERROR, LISTING_SOURCE_NOT_FOUND,
        LISTING_SOURCE_TEMPORARILY_UNAVAILABLE, PARTY_NOT_FOUND,
    },
    state::SourceLocationDefaultsState,
    wire::parse_path_object_id,
};
use aura_historia_contracts::listing_sources::location_default::{
    AssignData, ChangeData, ClearData, DefaultData,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::ListingSourceId;
use listing_source_service::use_cases::location_default::*;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid source location default input.")
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
fn assign_command(
    source_id: ListingSourceId,
    body: &str,
) -> Result<AssignLocationDefaultCommand, ApiError> {
    parse::<AssignData>(body)?
        .into_command(source_id)
        .map_err(|_| bad())
}
fn change_command(
    source_id: ListingSourceId,
    body: &str,
) -> Result<ChangeLocationDefaultApplicabilityCommand, ApiError> {
    parse::<ChangeData>(body)?
        .into_command(source_id)
        .map_err(|_| bad())
}
fn response(result: Result<LocationDefaultView, LocationDefaultError>) -> Response {
    match result {
        Ok(v) => Json(DefaultData::from(v)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
impl From<LocationDefaultError> for ApiError {
    fn from(error: LocationDefaultError) -> Self {
        match error {
            LocationDefaultError::AuthenticationRequired => Self::unauthorized(INVALID_CREDENTIALS),
            LocationDefaultError::Forbidden => Self::forbidden(FORBIDDEN),
            LocationDefaultError::LocationNotFound => Self::not_found(PARTY_NOT_FOUND),
            LocationDefaultError::SourceNotFound => Self::not_found(LISTING_SOURCE_NOT_FOUND),
            LocationDefaultError::InvalidRevision | LocationDefaultError::InvalidAssignment(_) => {
                bad()
            }
            LocationDefaultError::LocationRetired
            | LocationDefaultError::DefaultNotAssigned
            | LocationDefaultError::ConcurrencyConflict
            | LocationDefaultError::RevisionExhausted => Self::conflict(CONFLICT).with_detail(
                "Source location default cannot be changed at this revision or eligibility.",
            ),
            LocationDefaultError::TemporarilyUnavailable { .. }
            | LocationDefaultError::Transaction(_) => {
                Self::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
            }
            LocationDefaultError::InvalidPersistedState { .. }
            | LocationDefaultError::Internal { .. } => {
                Self::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
            }
        }
    }
}
pub async fn get(
    State(s): State<SourceLocationDefaultsState>,
    Path(source): Path<String>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    match parse_path_object_id(&source, "listing_source_id", "ListingSource") {
        Ok(source_id) => response(
            s.get
                .execute(&ctx, GetLocationDefaultRequest { source_id })
                .await,
        ),
        Err(e) => e.into_response(),
    }
}
pub async fn assign(
    State(s): State<SourceLocationDefaultsState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = parse_path_object_id(&source, "listing_source_id", "ListingSource")
        .and_then(|id| assign_command(id, &body));
    match command {
        Ok(c) => response(s.assign.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}
pub async fn clear(
    State(s): State<SourceLocationDefaultsState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let d: ClearData = parse(&body)?;
        Ok::<_, ApiError>(d.into_command(parse_path_object_id(
            &source,
            "listing_source_id",
            "ListingSource",
        )?))
    })();
    match command {
        Ok(c) => response(s.clear.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}
pub async fn change(
    State(s): State<SourceLocationDefaultsState>,
    Path(source): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = parse_path_object_id(&source, "listing_source_id", "ListingSource")
        .and_then(|id| change_command(id, &body));
    match command {
        Ok(c) => response(s.change.execute(&ctx, c).await),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests;
