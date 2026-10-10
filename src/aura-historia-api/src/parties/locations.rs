use crate::{
    auth::{OptionalAuthExtractor, protected_context, request_metadata},
    error::{ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, BAD_QUERY_PARAMETER_VALUE},
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
use geo::{
    AddressText, GeoPoint, GeographicDescription, PostalCode, SpatialPosition, SpatialPrecision,
    SubdivisionCode, country_from_code, reference::ReferenceRelease,
};
use party_core::{
    party_id::PartyId,
    party_location::{
        PartyLocationContent, PartyLocationDisclosure, PartyLocationLabel, PartyLocationLifecycle,
        PartyLocationRole,
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeographyData {
    reference_release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdivision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    postal_code: Option<String>,
}
impl GeographyData {
    fn into_core(self) -> Result<GeographicDescription, ApiError> {
        let release = ReferenceRelease::from_code(&self.reference_release).map_err(|_| bad())?;
        GeographicDescription::new_in_release(
            release,
            self.address_text
                .map(AddressText::new)
                .transpose()
                .map_err(|_| bad())?,
            self.country
                .map(|v| country_from_code(&v, release))
                .transpose()
                .map_err(|_| bad())?,
            self.subdivision
                .map(|v| SubdivisionCode::from_code(&v, release))
                .transpose()
                .map_err(|_| bad())?,
            self.postal_code
                .map(PostalCode::new)
                .transpose()
                .map_err(|_| bad())?,
        )
        .map_err(|_| bad())?
        .ok_or_else(bad)
    }
}
impl From<GeographicDescription> for GeographyData {
    fn from(v: GeographicDescription) -> Self {
        Self {
            reference_release: v.reference_release().as_str().to_owned(),
            address_text: v.address_text().map(|v| v.as_str().to_owned()),
            country: v.country().map(|v| v.alpha2().to_owned()),
            subdivision: v.subdivision().map(|v| v.as_str().to_owned()),
            postal_code: v.postal_code().map(|v| v.as_str().to_owned()),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PositionData {
    latitude_degrees: f64,
    longitude_degrees: f64,
    precision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accuracy_metres: Option<f64>,
}
impl PositionData {
    fn into_core(self) -> Result<SpatialPosition, ApiError> {
        SpatialPosition::new(
            GeoPoint::new(self.latitude_degrees, self.longitude_degrees).map_err(|_| bad())?,
            SpatialPrecision::from_code(&self.precision).ok_or_else(bad)?,
            self.accuracy_metres,
        )
        .map_err(|_| bad())
    }
}
impl From<SpatialPosition> for PositionData {
    fn from(v: SpatialPosition) -> Self {
        Self {
            latitude_degrees: v.point().latitude_degrees(),
            longitude_degrees: v.point().longitude_degrees(),
            precision: v.precision().as_str().to_owned(),
            accuracy_metres: v.accuracy_metres(),
        }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    roles: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    geography: Option<GeographyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<PositionData>,
    disclosure: &'static str,
    lifecycle: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<PartyLocationRevision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<EvidenceData>,
}
impl From<PartyLocationView> for LocationData {
    fn from(v: PartyLocationView) -> Self {
        Self {
            id: v.id,
            party_id: v.party_id,
            label: v.label.map(|v| v.as_str().to_owned()),
            roles: v.roles.iter().map(|v| v.as_str()).collect(),
            geography: v.geography.map(Into::into),
            position: v.position.map(Into::into),
            disclosure: v.disclosure.as_str(),
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
    disclosure: PatchValue<String>,
    #[serde(default)]
    evidence: Option<EvidenceData>,
    #[serde(default)]
    relocates: Option<ExpectedData>,
}
fn create_command(
    party_id: PartyId,
    headers: &HeaderMap,
    body: &str,
) -> Result<CreatePartyLocationCommand, ApiError> {
    let d: CreateData = parse(body)?;
    let mut keys = headers.get_all("idempotency-key").iter();
    let key_error = || {
        ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field("Idempotency-Key")
            .with_detail("Exactly one valid Idempotency-Key is required.")
    };
    let key = keys
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or_else(key_error)?;
    if keys.next().is_some() {
        return Err(key_error());
    }
    let disclosure = match non_nullable_patch(d.disclosure, "disclosure")? {
        PatchField::Set(v) => PartyLocationDisclosure::from_code(&v).ok_or_else(bad)?,
        _ => PartyLocationDisclosure::Private,
    };
    Ok(CreatePartyLocationCommand {
        party_id,
        idempotency_key: PartyLocationIdempotencyKey::new(key.to_owned())
            .map_err(|_| key_error())?,
        content: PartyLocationContent {
            label: PartyLocationLabel::new(d.label).map_err(|_| bad())?,
            roles: role_set(d.roles)?,
            geography: d.geography.map(GeographyData::into_core).transpose()?,
            position: d.position.map(PositionData::into_core).transpose()?,
            disclosure,
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
    disclosure: PatchValue<String>,
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
        geography: map_patch(clearable(d.geography), GeographyData::into_core)?,
        position: map_patch(clearable(d.position), PositionData::into_core)?,
        disclosure: map_patch(non_nullable_patch(d.disclosure, "disclosure")?, |v| {
            PartyLocationDisclosure::from_code(&v).ok_or_else(bad)
        })?,
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
            let location = format!("/api/v1/parties/{}/locations/{}", v.party_id, v.id);
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
        create_command(party_id, &headers, &body)
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
    private: bool,
) -> Response {
    let request = (|| {
        Ok::<_, ApiError>(GetPartyLocationRequest {
            party_id: parse_path_object_id(&party, "party_id", "Party")?,
            id: parse_path_object_id(&id, "location_id", "PartyLocation")?,
            private,
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
    get_location(s, party, id, ctx, true).await
}
pub async fn get_public(
    State(s): State<PartyLocationsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(s.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    get_location(s, party, id, principal.operation_context(metadata), false).await
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
    private: bool,
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
            private,
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
    list_locations(s, party, query, ctx, true).await
}
pub async fn list_public(
    State(s): State<PartyLocationsState>,
    Path(party): Path<String>,
    headers: HeaderMap,
    query: Result<Query<ListData>, QueryRejection>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(s.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    list_locations(
        s,
        party,
        query,
        principal.operation_context(metadata),
        false,
    )
    .await
}

pub async fn no_store_response(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_unresolved_sites_privately_without_any_geocoder() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("agent-command-1"),
        );
        for body in [
            r#"{"label":"Store"}"#,
            r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE"}}"#,
            r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Unknown road"}}"#,
        ] {
            let c = create_command(PartyId::new(), &headers, body).unwrap();
            assert_eq!(PartyLocationDisclosure::Private, c.content.disclosure);
            assert!(c.content.position.is_none());
        }
        for body in [
            r#"{"label":"Store","disclosure":null}"#,
            r#"{"label":"Store","trust":"ADMIN"}"#,
            r#"{"label":"Store","evidence":{"reference":"x","scope":"LISTING_INVENTORY"}}"#,
            r#"{"label":"Store","geography":{"referenceRelease":"iso-codes-4.20.1","country":"de"}}"#,
            r#"{"label":"Store","roles":["WAREHOUSE","WAREHOUSE"]}"#,
        ] {
            assert!(create_command(PartyId::new(), &headers, body).is_err());
        }
        assert!(create_command(PartyId::new(), &HeaderMap::new(), r#"{"label":"Store"}"#).is_err());
    }
    #[test]
    fn maps_omitted_set_clear_and_requires_same_site_assertion() {
        let party = PartyId::new();
        let id = PartyLocationId::new();
        let c = update_command(party,id,r#"{"expectedRevision":1,"correction":"SAME_SITE","geography":null,"position":null,"evidence":null,"roles":[]}"#).unwrap();
        assert_eq!(PatchField::Unchanged, c.label);
        assert_eq!(PatchField::Clear, c.geography);
        assert_eq!(PatchField::Clear, c.position);
        assert_eq!(PatchField::Clear, c.evidence);
        assert_eq!(PatchField::Set(BTreeSet::new()), c.roles);
        let c = update_command(party,id,r#"{"expectedRevision":1,"correction":"SAME_SITE","label":"Corrected","disclosure":"COARSE_PUBLIC"}"#).unwrap();
        assert!(matches!(c.label, PatchField::Set(_)));
        for body in [
            r#"{}"#,
            r#"{"expectedRevision":0,"correction":"SAME_SITE"}"#,
            r#"{"expectedRevision":1,"correction":"RELOCATION"}"#,
            r#"{"expectedRevision":1,"correction":"SAME_SITE","label":null}"#,
            r#"{"expectedRevision":1,"correction":"SAME_SITE","roles":null}"#,
            r#"{"expectedRevision":1,"correction":"SAME_SITE","disclosure":null}"#,
        ] {
            assert!(update_command(party, id, body).is_err());
        }
    }
    #[test]
    fn ids_and_positions_use_validated_constructors() {
        let id = PartyLocationId::new();
        assert_eq!(
            id,
            serde_json::from_value::<PartyLocationId>(serde_json::json!(id)).unwrap()
        );
        assert!(
            serde_json::from_value::<PartyLocationId>(serde_json::json!(id.as_uuid())).is_err()
        );
        for latitude in [91.0, f64::INFINITY, f64::NAN] {
            assert!(
                PositionData {
                    latitude_degrees: latitude,
                    longitude_degrees: 0.0,
                    precision: "PREMISES".to_owned(),
                    accuracy_metres: None
                }
                .into_core()
                .is_err()
            );
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use crate::{
        auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
        state::AppState,
    };
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;
    #[derive(Default)]
    struct Capture {
        calls: Mutex<Vec<&'static str>>,
    }
    impl Capture {
        fn view(&self, party_id: PartyId, id: PartyLocationId, private: bool) -> PartyLocationView {
            PartyLocationView {
                id,
                party_id,
                label: private.then(|| PartyLocationLabel::new("Site".to_owned()).unwrap()),
                roles: BTreeSet::new(),
                geography: None,
                position: None,
                disclosure: PartyLocationDisclosure::CoarsePublic,
                lifecycle: PartyLocationLifecycle::Active,
                revision: private.then_some(PartyLocationRevision::INITIAL),
                input_revision: private.then_some(1),
                evidence: private.then(|| {
                    LocationEvidence::new(
                        "Private reference".to_owned(),
                        None,
                        LocationAssertionScope::SiteDescription,
                    )
                    .unwrap()
                }),
            }
        }
    }
    #[async_trait::async_trait]
    impl CreatePartyLocationUseCase for Capture {
        async fn execute(
            &self,
            c: &OperationContext,
            v: CreatePartyLocationCommand,
        ) -> Result<PartyLocationView, PartyLocationError> {
            assert!(matches!(
                c.principal,
                application::operation_context::Principal::DelegatedUser { .. }
            ));
            assert!(!c.request_id.to_string().is_empty());
            assert_ne!("missing-request-id", c.request_id.to_string());
            assert_eq!("command-123", v.idempotency_key.as_str());
            self.calls.lock().unwrap().push("create");
            Ok(self.view(v.party_id, PartyLocationId::new(), true))
        }
    }
    #[async_trait::async_trait]
    impl UpdatePartyLocationUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            v: UpdatePartyLocationCommand,
        ) -> Result<PartyLocationView, PartyLocationError> {
            assert_eq!(PatchField::Clear, v.position);
            self.calls.lock().unwrap().push("correct");
            Ok(self.view(v.party_id, v.expected.id, true))
        }
    }
    #[async_trait::async_trait]
    impl SetPartyLocationLifecycleUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            v: SetPartyLocationLifecycleCommand,
        ) -> Result<PartyLocationView, PartyLocationError> {
            self.calls.lock().unwrap().push("lifecycle");
            let mut view = self.view(v.party_id, v.expected.id, true);
            view.lifecycle = v.lifecycle;
            Ok(view)
        }
    }
    #[async_trait::async_trait]
    impl GrantPartyLocationManagementUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            _: GrantPartyLocationManagementCommand,
        ) -> Result<(), PartyLocationError> {
            self.calls.lock().unwrap().push("grant");
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl GetPartyLocationUseCase for Capture {
        async fn execute(
            &self,
            c: &OperationContext,
            v: GetPartyLocationRequest,
        ) -> Result<PartyLocationView, PartyLocationError> {
            if !v.private {
                assert!(matches!(
                    c.principal,
                    application::operation_context::Principal::Anonymous
                        | application::operation_context::Principal::DelegatedUser { .. }
                ));
            }
            self.calls.lock().unwrap().push("get");
            Ok(self.view(v.party_id, v.id, v.private))
        }
    }
    #[async_trait::async_trait]
    impl ListPartyLocationsUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            v: ListPartyLocationsRequest,
        ) -> Result<PartyLocationsPage, PartyLocationError> {
            self.calls.lock().unwrap().push("list");
            Ok(PartyLocationsPage {
                items: vec![self.view(v.party_id, PartyLocationId::new(), v.private)],
                next: None,
            })
        }
    }
    struct Auth;
    #[async_trait::async_trait]
    impl TokenAuthenticator for Auth {
        async fn authenticate(
            &self,
            token: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            if token != "test-token" {
                return Err(AuthError::InvalidCredentials);
            }
            Ok(TransportPrincipal::User {
                user_id: UserId::new(),
                auth_method: AuthMethod::AuraAccessToken,
                capabilities: BTreeSet::from([
                    application::operation_context::CredentialCapability::PartiesRead,
                    application::operation_context::CredentialCapability::PartiesWrite,
                ]),
            })
        }
    }
    #[tokio::test]
    async fn routes_map_commands_authentication_redaction_and_cache_headers() {
        let capture = Arc::new(Capture::default());
        let state = PartyLocationsState {
            create: capture.clone(),
            update: capture.clone(),
            lifecycle: capture.clone(),
            grant: capture.clone(),
            get: capture.clone(),
            list: capture.clone(),
            authenticator: Arc::new(Auth),
        };
        let app = crate::app(AppState::new().with_party_locations(state));
        let party = PartyId::new();
        let id = PartyLocationId::new();
        let base = format!("/api/v1/parties/{party}/locations");
        let cases = [
            (
                "POST",
                base.clone(),
                r#"{"label":"Site"}"#,
                true,
                StatusCode::CREATED,
            ),
            (
                "PATCH",
                format!("{base}/{id}"),
                r#"{"expectedRevision":1,"correction":"SAME_SITE","position":null}"#,
                true,
                StatusCode::OK,
            ),
            (
                "PUT",
                format!("{base}/{id}/lifecycle"),
                r#"{"expectedRevision":1,"lifecycle":"RETIRED"}"#,
                true,
                StatusCode::OK,
            ),
            ("GET", format!("{base}/{id}"), "", true, StatusCode::OK),
            ("GET", base.clone(), "", true, StatusCode::OK),
            (
                "PUT",
                format!(
                    "/api/v1/admin/parties/{party}/location-managers/{}",
                    UserId::new()
                ),
                r#"{"granted":true}"#,
                true,
                StatusCode::NO_CONTENT,
            ),
            (
                "GET",
                format!("/api/v1/public/parties/{party}/locations/{id}"),
                "",
                false,
                StatusCode::OK,
            ),
            (
                "GET",
                format!("/api/v1/public/parties/{party}/locations"),
                "",
                false,
                StatusCode::OK,
            ),
            (
                "GET",
                format!("/api/v1/public/parties/{party}/locations/{id}"),
                "",
                true,
                StatusCode::OK,
            ),
            (
                "GET",
                format!("/api/v1/public/parties/{party}/locations"),
                "",
                true,
                StatusCode::OK,
            ),
            (
                "POST",
                base.clone(),
                r#"{"label":"Site"}"#,
                false,
                StatusCode::UNAUTHORIZED,
            ),
            (
                "PATCH",
                format!("{base}/{id}"),
                r#"{"expectedRevision":1,"correction":"SAME_SITE","label":null}"#,
                true,
                StatusCode::BAD_REQUEST,
            ),
        ];
        for (method, path, body, authenticated, status) in cases {
            let public = path.contains("/public/");
            let mut request = Request::builder()
                .method(method)
                .uri(&path)
                .header("content-type", "application/json")
                .header("idempotency-key", "command-123")
                .header("x-request-id", "request-123");
            if authenticated {
                request = request.header("authorization", "Bearer test-token");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            assert_eq!(status, response.status(), "{method} {path}");
            assert_eq!(
                "no-store",
                response.headers().get(header::CACHE_CONTROL).unwrap()
            );
            if status == StatusCode::CREATED {
                assert!(
                    response
                        .headers()
                        .get(header::LOCATION)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .contains("/locations/ploc_")
                );
            }
            if public {
                let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let v = if let Some(items) = v.get("items") {
                    &items[0]
                } else {
                    &v
                };
                assert!(v.get("evidence").is_none());
                assert!(v.get("revision").is_none());
                assert!(v.get("position").is_none());
                assert!(v.get("label").is_none());
            }
        }
        assert_eq!(10, capture.calls.lock().unwrap().len());
        for path in [
            format!("/api/v1/public/parties/{party}/locations/{id}"),
            format!("/api/v1/public/parties/{party}/locations"),
        ] {
            for credential in ["Bearer invalid-token", "Basic invalid-token"] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(&path)
                            .header("authorization", credential)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(StatusCode::UNAUTHORIZED, response.status());
                assert_eq!(
                    "no-store",
                    response.headers().get(header::CACHE_CONTROL).unwrap()
                );
            }
        }
        assert_eq!(10, capture.calls.lock().unwrap().len());
    }
}
