use super::*;
use crate::{
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::AppState,
};
use application::patch_field::PatchField;
use axum::{body::Body, http::Request};
use party_core::party_location::{PartyLocationLabel, PartyLocationLifecycle};
use party_service::ports::party_location::PartyLocationRevision;
use party_service::use_cases::queries::list_party_location_managers::{
    ListPartyLocationManagersRequest, ListPartyLocationManagersUseCase, PartyLocationManagerAccess,
    PartyLocationManagerView, PartyLocationManagersPage,
};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
#[derive(Default)]
struct Capture {
    calls: Mutex<Vec<&'static str>>,
    manager_error: Mutex<Option<PartyLocationError>>,
}
impl Capture {
    fn view(&self, party_id: PartyId, id: PartyLocationId) -> PartyLocationView {
        PartyLocationView {
            id,
            party_id,
            label: PartyLocationLabel::new("Site".to_owned()).unwrap(),
            roles: BTreeSet::new(),
            geography: None,
            position: None,
            lifecycle: PartyLocationLifecycle::Active,
            revision: PartyLocationRevision::INITIAL,
            input_revision: 1,
            evidence: Some(
                LocationEvidence::new(
                    "Private reference".to_owned(),
                    None,
                    LocationAssertionScope::SiteDescription,
                )
                .unwrap(),
            ),
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
        self.calls.lock().unwrap().push("create");
        Ok(self.view(v.party_id, PartyLocationId::new()))
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
        Ok(self.view(v.party_id, v.expected.id))
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
        let mut view = self.view(v.party_id, v.expected.id);
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
        assert!(matches!(
            c.principal,
            application::operation_context::Principal::DelegatedUser { .. }
        ));
        self.calls.lock().unwrap().push("get");
        Ok(self.view(v.party_id, v.id))
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
            items: vec![self.view(v.party_id, PartyLocationId::new())],
            next: None,
        })
    }
}
struct Auth;
#[async_trait::async_trait]
impl ListPartyLocationManagersUseCase for Capture {
    async fn execute(
        &self,
        _: &OperationContext,
        request: ListPartyLocationManagersRequest,
    ) -> Result<PartyLocationManagersPage, PartyLocationError> {
        self.calls.lock().unwrap().push("list_managers");
        if let Some(error) = self.manager_error.lock().unwrap().take() {
            return Err(error);
        }
        let user_id = request.user_id.unwrap_or_default();
        Ok(PartyLocationManagersPage {
            items: vec![PartyLocationManagerView {
                user_id,
                email: serde_email::Email::try_from("manager@example.test").unwrap(),
                first_name: Some(user_core::first_name::FirstName::from("Ada")),
                last_name: None,
                access: vec![
                    PartyLocationManagerAccess::Explicit,
                    PartyLocationManagerAccess::Partnership {
                        partnership_ids: vec![
                            partnership_core::partnership_id::PartnershipId::new(),
                        ],
                    },
                ],
            }],
            cursor: application::pagination::Cursor {
                size: request.cursor.size,
                search_after: None,
            },
        })
    }
}
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
fn location_app(capture: Arc<Capture>) -> axum::Router {
    let state = PartyLocationsState {
        discovery: None,
        create: capture.clone(),
        update: capture.clone(),
        lifecycle: capture.clone(),
        grant: capture.clone(),
        get: capture.clone(),
        list: capture.clone(),
        list_managers: capture.clone(),
        authenticator: Arc::new(Auth),
    };
    crate::app(AppState::new().with_party_locations(state))
}
#[tokio::test]
async fn creates_without_a_key_and_ignores_legacy_headers() {
    let capture = Arc::new(Capture::default());
    let app = location_app(capture.clone());
    let path = format!("/v1/parties/{}/locations", PartyId::new());
    let mut ids = BTreeSet::new();
    for keys in [
        vec![],
        vec![HeaderValue::from_static("legacy-key")],
        vec![HeaderValue::from_static("legacy-key")],
        vec![HeaderValue::from_static("")],
        vec![HeaderValue::from_static("bad key")],
        vec![HeaderValue::from_static("one,two")],
        vec![
            HeaderValue::from_static("one"),
            HeaderValue::from_static("two"),
        ],
        vec![HeaderValue::from_bytes(&[0xff]).unwrap()],
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri(&path)
            .header("content-type", "application/json")
            .header("authorization", "Bearer test-token");
        for key in keys {
            request = request.header("idempotency-key", key);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(r#"{"label":"Site"}"#)).unwrap())
            .await
            .unwrap();
        assert_eq!(StatusCode::CREATED, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        assert!(!response.headers().contains_key("idempotency-key"));
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let id = body["id"].as_str().unwrap();
        assert_eq!(format!("{path}/{id}"), location);
        assert!(ids.insert(id.to_owned()));
    }
    assert_eq!(8, capture.calls.lock().unwrap().len());
}
#[tokio::test]
async fn should_not_resolve_or_redirect_legacy_party_location_routes() {
    let capture = Arc::new(Capture::default());
    let app = location_app(capture.clone());
    let party = PartyId::new();
    let id = PartyLocationId::new();
    let base = format!("/api/v1/parties/{party}/locations");
    for (method, path) in [
        ("GET", base.clone()),
        ("POST", base.clone()),
        ("GET", format!("{base}/{id}")),
        ("PATCH", format!("{base}/{id}")),
        ("PUT", format!("{base}/{id}/lifecycle")),
        (
            "GET",
            format!("/api/v1/admin/parties/{party}/location-managers"),
        ),
        (
            "PUT",
            format!(
                "/api/v1/admin/parties/{party}/location-managers/{}",
                UserId::new()
            ),
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"label":"Site"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(StatusCode::NOT_FOUND, response.status(), "{method} {path}");
        assert!(
            !response.headers().contains_key(header::LOCATION),
            "{method} {path}"
        );
    }
    assert!(capture.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn routes_map_commands_authentication_and_cache_headers() {
    let capture = Arc::new(Capture::default());
    let app = location_app(capture.clone());
    let party = PartyId::new();
    let id = PartyLocationId::new();
    let base = format!("/v1/parties/{party}/locations");
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
                "/v1/admin/parties/{party}/location-managers/{}",
                UserId::new()
            ),
            r#"{"granted":true}"#,
            true,
            StatusCode::NO_CONTENT,
        ),
        (
            "GET",
            format!("{base}/{id}"),
            "",
            false,
            StatusCode::UNAUTHORIZED,
        ),
        ("GET", base.clone(), "", false, StatusCode::UNAUTHORIZED),
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
    }
    assert_eq!(6, capture.calls.lock().unwrap().len());
    for path in [
        format!("/v1/public/parties/{party}/locations/{id}"),
        format!("/v1/public/parties/{party}/locations"),
    ] {
        for credential in [
            None,
            Some("Bearer test-token"),
            Some("Bearer invalid-token"),
            Some("Basic invalid-token"),
        ] {
            let mut request = Request::builder().uri(&path);
            if let Some(credential) = credential {
                request = request.header("authorization", credential);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(StatusCode::NOT_FOUND, response.status());
        }
    }
    assert_eq!(6, capture.calls.lock().unwrap().len());
}
#[tokio::test]
async fn manager_directory_maps_identity_validation_errors_and_cache_headers() {
    let capture = Arc::new(Capture::default());
    let app = location_app(capture.clone());
    let party = PartyId::new();
    let manager_path = format!("/v1/admin/parties/{party}/location-managers");
    let user_id = UserId::new();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "{manager_path}?userId={user_id}&access=EXPLICIT&size=1"
                ))
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(serde_json::json!(user_id), body["items"][0]["userId"]);
    assert_eq!("manager@example.test", body["items"][0]["email"]);
    assert_eq!("Ada", body["items"][0]["firstName"]);
    assert!(body["items"][0].get("lastName").is_none());
    assert_eq!(4, body["items"][0].as_object().unwrap().len());
    assert_eq!(
        serde_json::json!({"type":"EXPLICIT"}),
        body["items"][0]["access"][0]
    );
    assert_eq!("PARTNERSHIP", body["items"][0]["access"][1]["type"]);
    assert!(
        body["items"][0]["access"][1]["partnershipIds"][0]
            .as_str()
            .unwrap()
            .starts_with("psh_")
    );
    assert_eq!(1, body["size"]);
    assert!(body.get("searchAfter").is_none());
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
            PartyLocationError::TemporarilyUnavailable {
                source: application::error::static_error("test outage"),
            },
            StatusCode::SERVICE_UNAVAILABLE,
            "PARTY_TEMPORARILY_UNAVAILABLE",
        ),
    ] {
        *capture.manager_error.lock().unwrap() = Some(error);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&manager_path)
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(code, body["error"]);
    }
    let calls = capture.calls.lock().unwrap().len();
    for (path, credential, status) in [
        (manager_path.clone(), None, StatusCode::UNAUTHORIZED),
        (
            manager_path.clone(),
            Some("Bearer invalid-token"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            format!("{manager_path}?access=OTHER"),
            Some("Bearer test-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{manager_path}?userId=bad"),
            Some("Bearer test-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{manager_path}?searchAfter=bad"),
            Some("Bearer test-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{manager_path}?size=-1"),
            Some("Bearer test-token"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/admin/parties/bad/location-managers".to_owned(),
            Some("Bearer test-token"),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut request = Request::builder().uri(path);
        if let Some(credential) = credential {
            request = request.header("authorization", credential);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(status, response.status());
        assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
    }
    assert_eq!(calls, capture.calls.lock().unwrap().len());
}
