use crate::{
    auth::protected_context,
    error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE},
    patch_value::{PatchValue, clearable, non_nullable_patch},
    state::PartyLocationsState,
    wire::{parse_path_object_id, parse_query_object_id},
};
use application::{operation_context::OperationContext, patch_field::PatchField};
use axum::{
    Json,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use party_core::{
    party_id::PartyId,
    party_location::{
        PartyLocationContent, PartyLocationLabel, PartyLocationLifecycle, PartyLocationRole,
    },
    party_location_id::PartyLocationId,
};
use party_service::{
    ports::party_location::{ExpectedPartyLocation, PartyLocationRevision},
    use_cases::party_locations::*,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use user_core::user_id::UserId;

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid PartyLocation input.")
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
fn map_patch<T, U>(
    patch: PatchField<T>,
    convert: impl FnOnce(T) -> Result<U, ApiError>,
) -> Result<PatchField<U>, ApiError> {
    Ok(match patch {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Clear => PatchField::Clear,
        PatchField::Set(v) => PatchField::Set(convert(v)?),
    })
}

pub(crate) use aura_historia_contracts::geography::{GeographyData, PositionData};

impl From<aura_historia_contracts::geography::InvalidGeographyData> for ApiError {
    fn from(_: aura_historia_contracts::geography::InvalidGeographyData) -> Self {
        bad()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    observed_at: Option<OffsetDateTime>,
    scope: String,
}
impl EvidenceData {
    fn into_service(self) -> Result<LocationEvidence, ApiError> {
        LocationEvidence::new(
            self.reference,
            self.observed_at,
            LocationAssertionScope::from_code(&self.scope).ok_or_else(bad)?,
        )
        .map_err(|_| bad())
    }
}
impl From<LocationEvidence> for EvidenceData {
    fn from(v: LocationEvidence) -> Self {
        Self {
            reference: v.reference().to_owned(),
            observed_at: v.observed_at(),
            scope: v.scope().as_str().to_owned(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationData {
    id: PartyLocationId,
    party_id: PartyId,
    label: String,
    roles: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geography: Option<GeographyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<PositionData>,
    lifecycle: &'static str,
    revision: PartyLocationRevision,
    input_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<EvidenceData>,
}
impl From<PartyLocationView> for LocationData {
    fn from(v: PartyLocationView) -> Self {
        Self {
            id: v.id,
            party_id: v.party_id,
            label: v.label.as_str().to_owned(),
            roles: v.roles.iter().map(|v| v.as_str()).collect(),
            geography: v.geography.map(Into::into),
            position: v.position.map(Into::into),
            lifecycle: v.lifecycle.as_str(),
            revision: v.revision,
            input_revision: v.input_revision,
            evidence: v.evidence.map(Into::into),
        }
    }
}
fn role_set(roles: Vec<String>) -> Result<BTreeSet<PartyLocationRole>, ApiError> {
    if roles.len() > 4 {
        return Err(bad());
    }
    let count = roles.len();
    let set = roles
        .into_iter()
        .map(|v| PartyLocationRole::from_code(&v).ok_or_else(bad))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if count != set.len() {
        return Err(bad());
    }
    Ok(set)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedData {
    id: PartyLocationId,
    revision: PartyLocationRevision,
}
impl From<ExpectedData> for ExpectedPartyLocation {
    fn from(v: ExpectedData) -> Self {
        Self {
            id: v.id,
            revision: v.revision,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateData {
    label: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    geography: Option<GeographyData>,
    #[serde(default)]
    position: Option<PositionData>,
    #[serde(default)]
    evidence: Option<EvidenceData>,
    #[serde(default)]
    relocates: Option<ExpectedData>,
}
fn create_command(party_id: PartyId, body: &str) -> Result<CreatePartyLocationCommand, ApiError> {
    let d: CreateData = parse(body)?;
    Ok(CreatePartyLocationCommand {
        party_id,
        content: PartyLocationContent {
            label: PartyLocationLabel::new(d.label).map_err(|_| bad())?,
            roles: role_set(d.roles)?,
            geography: d.geography.map(GeographyData::into_core).transpose()?,
            position: d.position.map(PositionData::into_core).transpose()?,
        },
        evidence: d.evidence.map(EvidenceData::into_service).transpose()?,
        relocates: d.relocates.map(Into::into),
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateData {
    expected_revision: PartyLocationRevision,
    correction: String,
    #[serde(default)]
    label: PatchValue<String>,
    #[serde(default)]
    roles: PatchValue<Vec<String>>,
    #[serde(default)]
    geography: PatchValue<GeographyData>,
    #[serde(default)]
    position: PatchValue<PositionData>,
    #[serde(default)]
    evidence: PatchValue<EvidenceData>,
}
fn update_command(
    party_id: PartyId,
    id: PartyLocationId,
    body: &str,
) -> Result<UpdatePartyLocationCommand, ApiError> {
    let d: UpdateData = parse(body)?;
    if d.correction != "SAME_SITE" {
        return Err(bad());
    }
    Ok(UpdatePartyLocationCommand {
        party_id,
        expected: ExpectedPartyLocation {
            id,
            revision: d.expected_revision,
        },
        label: map_patch(non_nullable_patch(d.label, "label")?, |v| {
            PartyLocationLabel::new(v).map_err(|_| bad())
        })?,
        roles: map_patch(non_nullable_patch(d.roles, "roles")?, role_set)?,
        geography: map_patch(clearable(d.geography), |v| {
            v.into_core().map_err(Into::into)
        })?,
        position: map_patch(clearable(d.position), |v| v.into_core().map_err(Into::into))?,
        evidence: map_patch(clearable(d.evidence), EvidenceData::into_service)?,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleData {
    expected_revision: PartyLocationRevision,
    lifecycle: String,
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
        Ok::<_, ApiError>(SetPartyLocationLifecycleCommand {
            party_id,
            expected: ExpectedPartyLocation {
                id,
                revision: d.expected_revision,
            },
            lifecycle: PartyLocationLifecycle::from_code(&d.lifecycle).ok_or_else(bad)?,
        })
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListData {
    after: Option<String>,
    limit: Option<u32>,
}
async fn list_locations(
    s: PartyLocationsState,
    party: String,
    query: Result<Query<ListData>, QueryRejection>,
    context: OperationContext,
) -> Response {
    let request = (|| {
        let Query(d) = query.map_err(|_| ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE))?;
        if d.limit.is_some_and(|limit| !(1..=100).contains(&limit)) {
            return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE));
        }
        Ok::<_, ApiError>(ListPartyLocationsRequest {
            party_id: parse_path_object_id(&party, "party_id", "Party")?,
            after: d
                .after
                .map(|v| parse_query_object_id(&v, "after", "PartyLocation"))
                .transpose()?,
            limit: d.limit.unwrap_or(25),
        })
    })();
    let mut response = match request {
        Ok(request) => match s.list.execute(&context, request).await {
            Ok(page) => Json(serde_json::json!({
                "items": page.items.into_iter().map(LocationData::from).collect::<Vec<_>>(),
                "next": page.next,
            }))
            .into_response(),
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
