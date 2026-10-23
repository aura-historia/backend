use crate::{
    auth::protected_context,
    error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, INVALID_OBJECT_ID},
    state::PartyLocationsState,
    wire::parse_path_object_id,
};
use application::operation_context::OperationContext;
pub use aura_historia_contracts::party_locations::management::ListData;
use aura_historia_contracts::party_locations::management::{
    CreateData, LifecycleData, ListInputError, LocationData, LocationInputError, PageData,
    UpdateData,
};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use party_service::use_cases::party_locations::*;
use serde::Deserialize;
use user_core::user_id::UserId;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid PartyLocation input.")
}
impl From<aura_historia_contracts::geography::InvalidGeographyData> for ApiError {
    fn from(_: aura_historia_contracts::geography::InvalidGeographyData) -> Self {
        bad()
    }
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
impl From<LocationInputError> for ApiError {
    fn from(error: LocationInputError) -> Self {
        Self::bad_request(BAD_BODY_VALUE).with_detail(error.to_string())
    }
}
fn create_command(party_id: PartyId, body: &str) -> Result<CreatePartyLocationCommand, ApiError> {
    parse::<CreateData>(body)?
        .into_command(party_id)
        .map_err(Into::into)
}
fn update_command(
    party_id: PartyId,
    id: PartyLocationId,
    body: &str,
) -> Result<UpdatePartyLocationCommand, ApiError> {
    parse::<UpdateData>(body)?
        .into_command(party_id, id)
        .map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantData {
    granted: bool,
}

fn location_response(
    result: Result<PartyLocationView, PartyLocationError>,
    created: bool,
) -> Response {
    let mut response = match result {
        Ok(v) => {
            let location = format!("/v1/parties/{}/locations/{}", v.party_id, v.id);
            let mut r = (
                if created {
                    StatusCode::CREATED
                } else {
                    StatusCode::OK
                },
                Json(LocationData::from(v)),
            )
                .into_response();
            if created && let Ok(value) = HeaderValue::from_str(&location) {
                r.headers_mut().insert(header::LOCATION, value);
            }
            r
        }
        Err(e) => ApiError::from(e).into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub async fn create(
    State(s): State<PartyLocationsState>,
    Path(party): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let party_id = parse_path_object_id(&party, "party_id", "Party")?;
        create_command(party_id, &body)
    })();
    match command {
        Ok(c) => location_response(s.create.execute(&ctx, c).await, true),
        Err(e) => e.into_response(),
    }
}
pub async fn update(
    State(s): State<PartyLocationsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let party_id = parse_path_object_id(&party, "party_id", "Party")?;
        let id = parse_path_object_id(&id, "location_id", "PartyLocation")?;
        update_command(party_id, id, &body)
    })();
    match command {
        Ok(c) => location_response(s.update.execute(&ctx, c).await, false),
        Err(e) => e.into_response(),
    }
}
pub async fn lifecycle(
    State(s): State<PartyLocationsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let party_id = parse_path_object_id(&party, "party_id", "Party")?;
        let id = parse_path_object_id(&id, "location_id", "PartyLocation")?;
        let d: LifecycleData = parse(&body)?;
        d.into_command(party_id, id).map_err(ApiError::from)
    })();
    match command {
        Ok(c) => location_response(s.lifecycle.execute(&ctx, c).await, false),
        Err(e) => e.into_response(),
    }
}
pub async fn grant(
    State(s): State<PartyLocationsState>,
    Path((party, user)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let command = (|| {
        let party_id = parse_path_object_id(&party, "party_id", "Party")?;
        let user_id: UserId = parse_path_object_id(&user, "user_id", "User")?;
        let d: GrantData = parse(&body)?;
        Ok::<_, ApiError>(GrantPartyLocationManagementCommand {
            party_id,
            user_id,
            granted: d.granted,
        })
    })();
    match command {
        Ok(c) => match s.grant.execute(&ctx, c).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => ApiError::from(e).into_response(),
        },
        Err(e) => e.into_response(),
    }
}
async fn get_location(
    s: PartyLocationsState,
    party: String,
    id: String,
    context: OperationContext,
) -> Response {
    let request = (|| {
        Ok::<_, ApiError>(GetPartyLocationRequest {
            party_id: parse_path_object_id(&party, "party_id", "Party")?,
            id: parse_path_object_id(&id, "location_id", "PartyLocation")?,
        })
    })();
    match request {
        Ok(r) => location_response(s.get.execute(&context, r).await, false),
        Err(e) => e.into_response(),
    }
}
pub async fn get(
    State(s): State<PartyLocationsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    get_location(s, party, id, ctx).await
}
async fn list_locations(
    s: PartyLocationsState,
    party: String,
    query: Result<Query<ListData>, QueryRejection>,
    context: OperationContext,
) -> Response {
    let request = (|| {
        let Query(d) = query.map_err(|_| ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE))?;
        let party_id = parse_path_object_id(&party, "party_id", "Party")?;
        d.into_request(party_id).map_err(|error| match error {
            ListInputError::Limit => ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE),
            ListInputError::After(_) => ApiError::bad_request(INVALID_OBJECT_ID)
                .with_detail("must be a valid PartyLocation ID")
                .with_query_field("after"),
        })
    })();
    let mut response = match request {
        Ok(request) => match s.list.execute(&context, request).await {
            Ok(page) => Json(PageData::from(page)).into_response(),
            Err(error) => ApiError::from(error).into_response(),
        },
        Err(error) => error.into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub async fn list(
    State(s): State<PartyLocationsState>,
    Path(party): Path<String>,
    headers: HeaderMap,
    query: Result<Query<ListData>, QueryRejection>,
) -> Response {
    let (ctx, _) = match protected_context(s.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    list_locations(s, party, query, ctx).await
}

pub async fn no_store_response(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod route_tests;
