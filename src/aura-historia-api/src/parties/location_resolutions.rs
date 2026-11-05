//! Administrator review of PartyLocation geocoding. Candidate summaries carry normalized
//! geography and attribution only; provider references, regions and profiles stay private.
use crate::{
    auth::protected_context,
    error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE},
    state::PartyLocationResolutionsState,
    wire::{parse_path_object_id, parse_query_object_id},
};
use aura_historia_contracts::{
    geography::{GeographyData, PositionData},
    party_locations::management::LocationData,
};
use axum::{
    Json,
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use geo_service::resolution::{ResolutionCandidate, ResolutionId, ResolutionView, ResolutionWork};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use party_service::{
    ports::party_location::{ExpectedPartyLocation, PartyLocationRevision},
    use_cases::{party_location_resolution::*, party_locations::PartyLocationError},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DEFAULT_LIMIT: u32 = 25;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    status: Option<String>,
    after: Option<String>,
    limit: Option<String>,
}

fn bad_query(field: &'static str, detail: &'static str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
}

fn parse_list_query(
    raw: Option<&str>,
) -> Result<ListPartyLocationResolutionReviewsRequest, ApiError> {
    let query: ListQuery =
        crate::wire::from_query_str(raw.unwrap_or_default()).map_err(|error| {
            ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE).with_detail(error.to_string())
        })?;
    let status = query
        .status
        .as_deref()
        .map(|value| match value {
            "AMBIGUOUS" => Ok(ResolutionAttention::Ambiguous),
            "BLOCKED" => Ok(ResolutionAttention::Blocked),
            "FAILED" => Ok(ResolutionAttention::Failed),
            _ => Err(bad_query(
                "status",
                "Expected AMBIGUOUS, BLOCKED or FAILED.",
            )),
        })
        .transpose()?;
    let limit = query
        .limit
        .as_deref()
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .filter(|limit| (1..=100).contains(limit))
                .ok_or_else(|| bad_query("limit", "Page size must be from 1 through 100."))
        })
        .transpose()?
        .unwrap_or(DEFAULT_LIMIT);
    Ok(ListPartyLocationResolutionReviewsRequest {
        status,
        after: query
            .after
            .as_deref()
            .map(|value| parse_query_object_id(value, "after", "PartyLocation"))
            .transpose()?,
        limit,
    })
}

fn parse_body<T: DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    serde_json::from_str(body).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid PartyLocation resolution input.")
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestData {
    expected_revision: PartyLocationRevision,
}

fn work(id: ResolutionId, generation: u64) -> Result<ResolutionWork, ApiError> {
    if generation == 0 {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Resolution generation must be positive."));
    }
    Ok(ResolutionWork { id, generation })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptCandidateData {
    expected_revision: PartyLocationRevision,
    resolution_id: ResolutionId,
    generation: u64,
    candidate_index: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryData {
    expected_revision: PartyLocationRevision,
    resolution_id: ResolutionId,
    generation: u64,
    #[serde(default)]
    force: bool,
}

fn expected(id: PartyLocationId, revision: PartyLocationRevision) -> ExpectedPartyLocation {
    ExpectedPartyLocation { id, revision }
}

/// Normalized candidate geography; never the provider record or its external reference.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateData {
    index: usize,
    acceptable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    geography: Option<GeographyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<PositionData>,
    provider: String,
    attribution_url: String,
}
impl CandidateData {
    fn new(index: usize, candidate: ResolutionCandidate) -> Self {
        Self {
            index,
            acceptable: candidate.acceptable,
            geography: candidate.geography.description().cloned().map(Into::into),
            position: candidate.geography.position().map(Into::into),
            provider: candidate.provider,
            attribution_url: candidate.attribution_url,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolutionData {
    resolution_id: ResolutionId,
    generation: u64,
    location_revision: u64,
    input_revision: u64,
    status: &'static str,
    attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted_candidate_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_candidate_index: Option<usize>,
    candidates: Vec<CandidateData>,
}
impl From<ResolutionView> for ResolutionData {
    fn from(view: ResolutionView) -> Self {
        let (accepted, candidates) = view
            .evidence
            .map(|e| (e.accepted_index, e.candidates))
            .unwrap_or_default();
        Self {
            resolution_id: view.work.id,
            generation: view.work.generation,
            location_revision: view.guard,
            input_revision: view.input_revision,
            status: view.status.as_str(),
            attempts: view.attempts,
            failure: view.last_failure,
            accepted_candidate_index: accepted,
            selected_candidate_index: view.selected_candidate,
            candidates: candidates
                .into_iter()
                .enumerate()
                .map(|(index, candidate)| CandidateData::new(index, candidate))
                .collect(),
        }
    }
}

/// `resolution` is null when the location has no geocoding work: it carries an explicit
/// position, has no address text, or is retired.
#[derive(Serialize)]
struct LocationResolutionData {
    resolution: Option<ResolutionData>,
}

#[derive(Serialize)]
struct ReviewData {
    location: LocationData,
    resolution: ResolutionData,
}

#[derive(Serialize)]
struct ReviewPageData {
    items: Vec<ReviewData>,
    next: Option<PartyLocationId>,
}
impl From<PartyLocationResolutionReviewsPage> for ReviewPageData {
    fn from(page: PartyLocationResolutionReviewsPage) -> Self {
        Self {
            items: page
                .items
                .into_iter()
                .map(|item| ReviewData {
                    location: item.location.into(),
                    resolution: item.resolution.into(),
                })
                .collect(),
            next: page.next,
        }
    }
}

fn resolution_response(result: Result<Option<ResolutionView>, PartyLocationError>) -> Response {
    match result {
        Ok(view) => (
            StatusCode::OK,
            Json(LocationResolutionData {
                resolution: view.map(Into::into),
            }),
        )
            .into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn location_path(party: &str, id: &str) -> Result<(PartyId, PartyLocationId), ApiError> {
    Ok((
        parse_path_object_id(party, "party_id", "Party")?,
        parse_path_object_id(id, "location_id", "PartyLocation")?,
    ))
}

async fn manage(
    state: &PartyLocationResolutionsState,
    headers: &HeaderMap,
    command: Result<PartyLocationResolutionCommand, ApiError>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match command {
        Ok(command) => resolution_response(state.manage.execute(&context, command).await),
        Err(error) => error.into_response(),
    }
}

pub async fn list(
    State(state): State<PartyLocationResolutionsState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let request = match parse_list_query(raw.as_deref()) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };
    match state.list.execute(&context, request).await {
        Ok(page) => Json(ReviewPageData::from(page)).into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub async fn inspect(
    State(state): State<PartyLocationResolutionsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let command = location_path(&party, &id)
        .map(|(party_id, id)| PartyLocationResolutionCommand::Inspect { party_id, id });
    manage(&state, &headers, command).await
}

pub async fn request(
    State(state): State<PartyLocationResolutionsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = location_path(&party, &id).and_then(|(party_id, id)| {
        let data: RequestData = parse_body(&body)?;
        Ok(PartyLocationResolutionCommand::Request {
            party_id,
            expected: expected(id, data.expected_revision),
        })
    });
    manage(&state, &headers, command).await
}

pub async fn accept_candidate(
    State(state): State<PartyLocationResolutionsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = location_path(&party, &id).and_then(|(party_id, id)| {
        let data: AcceptCandidateData = parse_body(&body)?;
        Ok(PartyLocationResolutionCommand::AcceptCandidate {
            party_id,
            expected: expected(id, data.expected_revision),
            work: work(data.resolution_id, data.generation)?,
            candidate_index: data.candidate_index,
        })
    });
    manage(&state, &headers, command).await
}

pub async fn retry(
    State(state): State<PartyLocationResolutionsState>,
    Path((party, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let command = location_path(&party, &id).and_then(|(party_id, id)| {
        let data: RetryData = parse_body(&body)?;
        Ok(PartyLocationResolutionCommand::Retry {
            party_id,
            expected: expected(id, data.expected_revision),
            work: work(data.resolution_id, data.generation)?,
            force: data.force,
        })
    });
    manage(&state, &headers, command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
        state::AppState,
    };
    use application::operation_context::{CredentialCapability, OperationContext};
    use axum::{body::Body, http::Request, http::header};
    use geo::{CountryCode, DerivedGeography, GeoPoint, GeographicDescription, SpatialPosition};
    use geo_service::resolution::{
        ResolutionCandidate, ResolutionEvidence, ResolutionScope, ResolutionStatus, ScopeKind,
    };
    use party_core::party_location::{PartyLocationLabel, PartyLocationLifecycle};
    use party_service::use_cases::party_locations::PartyLocationView;
    use serde_json::{Value, json};
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct Capture {
        commands: Mutex<Vec<PartyLocationResolutionCommand>>,
        lists: Mutex<Vec<ListPartyLocationResolutionReviewsRequest>>,
        error: Mutex<Option<PartyLocationError>>,
        unbound: Mutex<bool>,
    }
    fn view(work: ResolutionWork) -> ResolutionView {
        let point = SpatialPosition::new(
            GeoPoint::new(52.5, 13.4).unwrap(),
            geo::SpatialPrecision::Premises,
            None,
        )
        .unwrap();
        let candidate = ResolutionCandidate {
            geography: DerivedGeography::new(
                GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap(),
                Some(point),
            )
            .unwrap(),
            acceptable: true,
            provider: "AMAZON_LOCATION".into(),
            product: "PLACES_V2".into(),
            data_source: "private-data-source".into(),
            region: "private-region".into(),
            profile: "private-profile".into(),
            external_reference: "private-provider-reference".into(),
            attribution_url: "https://example.test/attribution".into(),
        };
        ResolutionView {
            scope: ResolutionScope::new(ScopeKind::Party, Some(uuid::Uuid::now_v7())).unwrap(),
            work,
            input_revision: 2,
            guard: 3,
            status: ResolutionStatus::Ambiguous,
            evidence: Some(ResolutionEvidence {
                status: ResolutionStatus::Ambiguous,
                candidates: vec![candidate.clone(), candidate],
                accepted_index: None,
            }),
            selected_candidate: Some(1),
            last_failure: None,
            attempts: 1,
        }
    }
    fn work() -> ResolutionWork {
        ResolutionWork {
            id: ResolutionId::new(),
            generation: 4,
        }
    }
    #[async_trait::async_trait]
    impl ManagePartyLocationResolutionUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            command: PartyLocationResolutionCommand,
        ) -> Result<Option<ResolutionView>, PartyLocationError> {
            self.commands.lock().unwrap().push(command);
            if let Some(error) = self.error.lock().unwrap().take() {
                return Err(error);
            }
            Ok((!*self.unbound.lock().unwrap()).then(|| view(work())))
        }
    }
    #[async_trait::async_trait]
    impl ListPartyLocationResolutionReviewsUseCase for Capture {
        async fn execute(
            &self,
            _: &OperationContext,
            request: ListPartyLocationResolutionReviewsRequest,
        ) -> Result<PartyLocationResolutionReviewsPage, PartyLocationError> {
            self.lists.lock().unwrap().push(request);
            Ok(PartyLocationResolutionReviewsPage {
                items: vec![PartyLocationResolutionReview {
                    location: PartyLocationView {
                        id: PartyLocationId::new(),
                        party_id: PartyId::new(),
                        label: PartyLocationLabel::new("Showroom".to_owned()).unwrap(),
                        roles: BTreeSet::new(),
                        geography: None,
                        position: None,
                        lifecycle: PartyLocationLifecycle::Active,
                        revision: PartyLocationRevision::try_from(3u64).unwrap(),
                        input_revision: 2,
                        evidence: None,
                    },
                    resolution: view(work()),
                }],
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
                    CredentialCapability::PartiesRead,
                    CredentialCapability::PartiesWrite,
                ]),
            })
        }
    }
    fn app(capture: &Arc<Capture>) -> axum::Router {
        crate::app(
            AppState::new().with_party_location_resolutions(PartyLocationResolutionsState {
                manage: capture.clone(),
                list: capture.clone(),
                authenticator: Arc::new(Auth),
            }),
        )
    }
    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        body: &str,
        authenticated: bool,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if authenticated {
            request = request.header("authorization", "Bearer test-token");
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap();
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn should_map_each_route_to_its_review_command() {
        let capture = Arc::new(Capture::default());
        let app = app(&capture);
        let (party, id, w) = (PartyId::new(), PartyLocationId::new(), work());
        let base = format!("/v1/admin/parties/{party}/locations/{id}/resolution");
        for (method, path, body) in [
            ("GET", base.clone(), String::new()),
            (
                "POST",
                format!("{base}/request"),
                r#"{"expectedRevision":3}"#.to_owned(),
            ),
            (
                "POST",
                format!("{base}/accept-candidate"),
                json!({"expectedRevision":3,"resolutionId":w.id,"generation":4,"candidateIndex":1})
                    .to_string(),
            ),
            (
                "POST",
                format!("{base}/retry"),
                json!({"expectedRevision":3,"resolutionId":w.id,"generation":4,"force":true})
                    .to_string(),
            ),
            (
                "POST",
                format!("{base}/retry"),
                json!({"expectedRevision":3,"resolutionId":w.id,"generation":4}).to_string(),
            ),
        ] {
            let (status, _) = call(&app, method, &path, &body, true).await;
            assert_eq!(StatusCode::OK, status, "{method} {path}");
        }
        let expected = ExpectedPartyLocation {
            id,
            revision: PartyLocationRevision::try_from(3u64).unwrap(),
        };
        let commands = capture.commands.lock().unwrap();
        assert!(matches!(
            commands[0],
            PartyLocationResolutionCommand::Inspect { party_id, id: target }
                if party_id == party && target == id
        ));
        assert!(matches!(
            commands[1],
            PartyLocationResolutionCommand::Request { party_id, expected: e }
                if party_id == party && e == expected
        ));
        assert!(matches!(
            commands[2],
            PartyLocationResolutionCommand::AcceptCandidate { expected: e, work, candidate_index: 1, .. }
                if e == expected && work == w
        ));
        assert!(matches!(
            commands[3],
            PartyLocationResolutionCommand::Retry { work, force: true, .. } if work == w
        ));
        assert!(matches!(
            commands[4],
            PartyLocationResolutionCommand::Retry { force: false, .. }
        ));
    }

    #[tokio::test]
    async fn should_expose_safe_candidate_summaries_and_null_unbound_resolutions() {
        let capture = Arc::new(Capture::default());
        let app = app(&capture);
        let path = format!(
            "/v1/admin/parties/{}/locations/{}/resolution",
            PartyId::new(),
            PartyLocationId::new()
        );
        let (_, body) = call(&app, "GET", &path, "", true).await;
        let resolution = &body["resolution"];
        assert!(
            resolution["resolutionId"]
                .as_str()
                .unwrap()
                .starts_with("gres_")
        );
        assert_eq!(json!(4), resolution["generation"]);
        assert_eq!(json!(3), resolution["locationRevision"]);
        assert_eq!(json!(2), resolution["inputRevision"]);
        assert_eq!("AMBIGUOUS", resolution["status"]);
        assert_eq!(json!(1), resolution["selectedCandidateIndex"]);
        assert!(resolution.get("acceptedCandidateIndex").is_none());
        assert!(resolution.get("failure").is_none());
        assert_eq!(
            json!({
                "index": 1,
                "acceptable": true,
                "geography": {"referenceRelease": "iso-codes-4.20.1", "country": "DE"},
                "position": {"latitudeDegrees": 52.5, "longitudeDegrees": 13.4, "precision": "PREMISES"},
                "provider": "AMAZON_LOCATION",
                "attributionUrl": "https://example.test/attribution"
            }),
            resolution["candidates"][1]
        );
        assert!(!body.to_string().contains("private"));
        *capture.unbound.lock().unwrap() = true;
        let (status, body) = call(&app, "GET", &path, "", true).await;
        assert_eq!(StatusCode::OK, status);
        assert_eq!(json!({"resolution": null}), body);
    }

    #[tokio::test]
    async fn should_page_the_review_queue_with_status_filter_and_keyset_cursor() {
        let capture = Arc::new(Capture::default());
        let app = app(&capture);
        let after = PartyLocationId::new();
        let (status, body) = call(
            &app,
            "GET",
            &format!("/v1/admin/party-location-resolutions?status=BLOCKED&after={after}&limit=100"),
            "",
            true,
        )
        .await;
        assert_eq!(StatusCode::OK, status);
        assert_eq!(Value::Null, body["next"]);
        assert_eq!("Showroom", body["items"][0]["location"]["label"]);
        assert_eq!("AMBIGUOUS", body["items"][0]["resolution"]["status"]);
        call(
            &app,
            "GET",
            "/v1/admin/party-location-resolutions",
            "",
            true,
        )
        .await;
        assert_eq!(
            vec![
                ListPartyLocationResolutionReviewsRequest {
                    status: Some(ResolutionAttention::Blocked),
                    after: Some(after),
                    limit: 100,
                },
                ListPartyLocationResolutionReviewsRequest {
                    status: None,
                    after: None,
                    limit: DEFAULT_LIMIT,
                },
            ],
            *capture.lists.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_or_unauthenticated_requests_before_the_use_case() {
        let capture = Arc::new(Capture::default());
        let app = app(&capture);
        let (party, id, w) = (PartyId::new(), PartyLocationId::new(), work());
        let base = format!("/v1/admin/parties/{party}/locations/{id}/resolution");
        let list = "/v1/admin/party-location-resolutions";
        for (method, path, body, authenticated, status) in [
            ("GET", base.clone(), String::new(), false, StatusCode::UNAUTHORIZED),
            ("GET", list.to_owned(), String::new(), false, StatusCode::UNAUTHORIZED),
            (
                "GET",
                format!("/v1/admin/parties/{party}/locations/{party}/resolution"),
                String::new(),
                true,
                StatusCode::BAD_REQUEST,
            ),
            ("POST", format!("{base}/request"), "{}".to_owned(), true, StatusCode::BAD_REQUEST),
            (
                "POST",
                format!("{base}/request"),
                r#"{"expectedRevision":0}"#.to_owned(),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                format!("{base}/accept-candidate"),
                json!({"expectedRevision":1,"resolutionId":w.id,"generation":0,"candidateIndex":0})
                    .to_string(),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                format!("{base}/accept-candidate"),
                json!({"expectedRevision":1,"resolutionId":w.id.into_uuid(),"generation":1,"candidateIndex":0})
                    .to_string(),
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "POST",
                format!("{base}/retry"),
                json!({"expectedRevision":1,"resolutionId":w.id,"generation":1,"candidateIndex":0})
                    .to_string(),
                true,
                StatusCode::BAD_REQUEST,
            ),
            ("GET", format!("{list}?status=PENDING"), String::new(), true, StatusCode::BAD_REQUEST),
            ("GET", format!("{list}?limit=0"), String::new(), true, StatusCode::BAD_REQUEST),
            ("GET", format!("{list}?limit=101"), String::new(), true, StatusCode::BAD_REQUEST),
            ("GET", format!("{list}?after={party}"), String::new(), true, StatusCode::BAD_REQUEST),
            ("GET", format!("{list}?size=10"), String::new(), true, StatusCode::BAD_REQUEST),
        ] {
            let (actual, _) = call(&app, method, &path, &body, authenticated).await;
            assert_eq!(status, actual, "{method} {path} {body}");
        }
        assert!(capture.commands.lock().unwrap().is_empty());
        assert!(capture.lists.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn should_map_review_failures_to_problems() {
        let capture = Arc::new(Capture::default());
        let app = app(&capture);
        let path = format!(
            "/v1/admin/parties/{}/locations/{}/resolution",
            PartyId::new(),
            PartyLocationId::new()
        );
        for (error, status, code) in [
            (
                PartyLocationError::Forbidden,
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
            ),
            (
                PartyLocationError::NotFound,
                StatusCode::NOT_FOUND,
                "PARTY_NOT_FOUND",
            ),
            (
                PartyLocationError::ConcurrencyConflict,
                StatusCode::CONFLICT,
                "CONFLICT",
            ),
            (
                PartyLocationError::InvalidInput,
                StatusCode::BAD_REQUEST,
                "BAD_BODY_VALUE",
            ),
        ] {
            *capture.error.lock().unwrap() = Some(error);
            let (actual, body) = call(&app, "GET", &path, "", true).await;
            assert_eq!(status, actual);
            assert_eq!(code, body["error"]);
        }
    }
}
