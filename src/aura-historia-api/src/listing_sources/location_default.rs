use crate::{
    auth::protected_context,
    error::{
        ApiError, BAD_BODY_VALUE, CONFLICT, FORBIDDEN, INVALID_CREDENTIALS,
        LISTING_SOURCE_INTERNAL_ERROR, LISTING_SOURCE_NOT_FOUND,
        LISTING_SOURCE_TEMPORARILY_UNAVAILABLE, PARTY_NOT_FOUND,
    },
    patch_value::{PatchValue, clearable},
    state::SourceLocationDefaultsState,
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use listing_source_core::{ListingSourceId, location_default::*};
use listing_source_service::{
    ports::location_default::LocationDefaultRevision, use_cases::location_default::*,
};
use party_core::party_location_id::PartyLocationId;
use serde::{Deserialize, Serialize};

fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid source location default input.")
}
fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| bad())
}
fn deserialize_revision<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<LocationDefaultRevision, D::Error> {
    let value = i64::deserialize(deserializer)?;
    LocationDefaultRevision::try_from(value).map_err(serde::de::Error::custom)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceData {
    reference: String,
    scope: String,
}
impl EvidenceData {
    fn into_core(self) -> Result<SourceLocationEvidence, ApiError> {
        SourceLocationEvidence::new(
            self.reference,
            SourceLocationMeaning::from_code(&self.scope).ok_or_else(bad)?,
        )
        .map_err(|_| bad())
    }
}
impl From<&SourceLocationEvidence> for EvidenceData {
    fn from(e: &SourceLocationEvidence) -> Self {
        Self {
            reference: e.reference().to_owned(),
            scope: e.scope().as_str().to_owned(),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignData {
    #[serde(deserialize_with = "deserialize_revision")]
    expected_revision: LocationDefaultRevision,
    party_location_id: PartyLocationId,
    business_association: Option<EvidenceData>,
    advertised_inventory: Option<EvidenceData>,
}
fn assign_command(
    source_id: ListingSourceId,
    body: &str,
) -> Result<AssignLocationDefaultCommand, ApiError> {
    let d: AssignData = parse(body)?;
    Ok(AssignLocationDefaultCommand {
        source_id,
        expected_revision: d.expected_revision,
        assignment: SourceLocationAssignment::new(
            d.party_location_id,
            d.business_association
                .map(EvidenceData::into_core)
                .transpose()?,
            d.advertised_inventory
                .map(EvidenceData::into_core)
                .transpose()?,
        )
        .map_err(|_| bad())?,
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClearData {
    #[serde(deserialize_with = "deserialize_revision")]
    expected_revision: LocationDefaultRevision,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangeData {
    #[serde(deserialize_with = "deserialize_revision")]
    expected_revision: LocationDefaultRevision,
    #[serde(default)]
    business_association: PatchValue<EvidenceData>,
    #[serde(default)]
    advertised_inventory: PatchValue<EvidenceData>,
}
fn evidence_patch(
    value: PatchValue<EvidenceData>,
) -> Result<application::patch_field::PatchField<SourceLocationEvidence>, ApiError> {
    use application::patch_field::PatchField;
    Ok(match clearable(value) {
        PatchField::Unchanged => PatchField::Unchanged,
        PatchField::Clear => PatchField::Clear,
        PatchField::Set(e) => PatchField::Set(e.into_core()?),
    })
}
fn change_command(
    source_id: ListingSourceId,
    body: &str,
) -> Result<ChangeLocationDefaultApplicabilityCommand, ApiError> {
    let d: ChangeData = parse(body)?;
    Ok(ChangeLocationDefaultApplicabilityCommand {
        source_id,
        expected_revision: d.expected_revision,
        business_association: evidence_patch(d.business_association)?,
        advertised_inventory: evidence_patch(d.advertised_inventory)?,
    })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssignmentData {
    party_location_id: PartyLocationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    business_association: Option<EvidenceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    advertised_inventory: Option<EvidenceData>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DefaultData {
    listing_source_id: ListingSourceId,
    revision: LocationDefaultRevision,
    assignment: Option<AssignmentData>,
    eligibility: Option<&'static str>,
}
fn response(result: Result<LocationDefaultView, LocationDefaultError>) -> Response {
    match result {
        Ok(v) => Json(DefaultData {
            listing_source_id: v.source_id,
            revision: v.revision,
            assignment: v.assignment.map(|a| AssignmentData {
                party_location_id: a.location_id(),
                business_association: a.business_association().map(Into::into),
                advertised_inventory: a.advertised_inventory().map(Into::into),
            }),
            eligibility: v.eligibility.map(|e| match e {
                SelectedLocationEligibility::Active => "ACTIVE",
                SelectedLocationEligibility::Retired => "RETIRED",
            }),
        })
        .into_response(),
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
        Ok::<_, ApiError>(ClearLocationDefaultCommand {
            source_id: parse_path_object_id(&source, "listing_source_id", "ListingSource")?,
            expected_revision: d.expected_revision,
        })
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
