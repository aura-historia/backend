use super::*;
use crate::{
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::AppState,
};
use application::{
    operation_context::{CredentialCapability, OperationContext, Principal},
    patch_field::PatchField,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[test]
fn validated_ids_evidence_scopes_and_patch_intent_are_required() {
    let source = ListingSourceId::new();
    let id = PartyLocationId::new();
    let body=serde_json::json!({"expectedRevision":1,"partyLocationId":id,"businessAssociation":{"reference":"Caller assertion","scope":"ASSOCIATED_BUSINESS"}}).to_string();
    let c = assign_command(source, &body).unwrap();
    assert_eq!(id, c.assignment.location_id());
    assert!(c.assignment.advertised_inventory().is_none());
    for invalid in [
        serde_json::json!({"expectedRevision":0,"partyLocationId":id}),
        serde_json::json!({"expectedRevision":i64::MAX as u64 + 1,"partyLocationId":id}),
        serde_json::json!({"expectedRevision":1,"partyLocationId":id.as_uuid()}),
        serde_json::json!({"expectedRevision":1,"partyLocationId":id,"sellerPartyId":"untrusted"}),
        serde_json::json!({"expectedRevision":1,"partyLocationId":id,"advertisedInventory":{"reference":"legal address","scope":"REGISTERED_ADDRESS"}}),
        serde_json::json!({"expectedRevision":1,"partyLocationId":id,"advertisedInventory":{"reference":"site","scope":"ASSOCIATED_BUSINESS"}}),
        serde_json::json!({"expectedRevision":1,"partyLocationId":id,"advertisedInventory":{"reference":" ","scope":"ADVERTISED_INVENTORY"}}),
    ] {
        assert!(assign_command(source, &invalid.to_string()).is_err());
    }
    let changed = change_command(
        source,
        r#"{"expectedRevision":2,"advertisedInventory":null}"#,
    )
    .unwrap();
    assert_eq!(PatchField::Clear, changed.advertised_inventory);
    assert_eq!(PatchField::Unchanged, changed.business_association);
    assert!(change_command(source, r#"{"expectedRevision":2,"partyLocationId":"x"}"#).is_err());
    let maximum = serde_json::json!({"expectedRevision":i64::MAX,"partyLocationId":id});
    assert_eq!(
        i64::MAX as u64,
        assign_command(source, &maximum.to_string())
            .unwrap()
            .expected_revision
            .into_inner()
    );
}
#[derive(Default)]
struct Capture {
    calls: Mutex<Vec<&'static str>>,
}
impl Capture {
    fn view(
        &self,
        source_id: ListingSourceId,
        assignment: Option<SourceLocationAssignment>,
    ) -> LocationDefaultView {
        LocationDefaultView {
            source_id,
            revision: LocationDefaultRevision::INITIAL,
            eligibility: assignment
                .as_ref()
                .map(|_| SelectedLocationEligibility::Active),
            assignment,
        }
    }
    fn record(&self, context: &OperationContext, action: &'static str) {
        assert!(matches!(context.principal, Principal::DelegatedUser { .. }));
        assert!(!context.request_id.to_string().is_empty());
        self.calls.lock().unwrap().push(action);
    }
}
#[async_trait::async_trait]
impl AssignLocationDefaultUseCase for Capture {
    async fn execute(
        &self,
        context: &OperationContext,
        c: AssignLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        self.record(context, "assign");
        assert_eq!(1, c.expected_revision.into_inner());
        Ok(self.view(c.source_id, Some(c.assignment)))
    }
}
#[async_trait::async_trait]
impl ClearLocationDefaultUseCase for Capture {
    async fn execute(
        &self,
        context: &OperationContext,
        c: ClearLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        self.record(context, "clear");
        assert_eq!(2, c.expected_revision.into_inner());
        Ok(self.view(c.source_id, None))
    }
}
#[async_trait::async_trait]
impl ChangeLocationDefaultApplicabilityUseCase for Capture {
    async fn execute(
        &self,
        context: &OperationContext,
        c: ChangeLocationDefaultApplicabilityCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        self.record(context, "change");
        assert_eq!(PatchField::Clear, c.advertised_inventory);
        assert_eq!(PatchField::Unchanged, c.business_association);
        Ok(self.view(c.source_id, None))
    }
}
#[async_trait::async_trait]
impl GetLocationDefaultUseCase for Capture {
    async fn execute(
        &self,
        context: &OperationContext,
        c: GetLocationDefaultRequest,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        self.record(context, "get");
        Ok(self.view(c.source_id, None))
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
            user_id: uuid::Uuid::now_v7().try_into().unwrap(),
            auth_method: AuthMethod::AuraAccessToken,
            capabilities: BTreeSet::from([
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ListingSourcesWrite,
            ]),
        })
    }
}
#[tokio::test]
async fn operator_and_admin_routes_map_commands_authentication_and_no_store() {
    let capture = Arc::new(Capture::default());
    let admin_capture = Arc::new(Capture::default());
    let state = |capture: &Arc<Capture>| SourceLocationDefaultsState {
        assign: capture.clone(),
        clear: capture.clone(),
        change: capture.clone(),
        get: capture.clone(),
        authenticator: Arc::new(Auth),
    };
    let app = crate::app(
        AppState::new()
            .with_source_location_defaults(state(&capture))
            .with_admin_source_location_defaults(state(&admin_capture)),
    );
    let id = ListingSourceId::new();
    let location = PartyLocationId::new();
    for (prefix, capture) in [("", &capture), ("/admin", &admin_capture)] {
        let path = format!("/v1{prefix}/listing-sources/{id}/location-default");
        let body=serde_json::json!({"expectedRevision":1,"partyLocationId":location,"businessAssociation":{"reference":"Private evidence","scope":"ASSOCIATED_BUSINESS"}}).to_string();
        let cases = [
            ("GET", "", true, StatusCode::OK),
            ("PUT", body.as_str(), true, StatusCode::OK),
            (
                "PATCH",
                r#"{"expectedRevision":2,"advertisedInventory":null}"#,
                true,
                StatusCode::OK,
            ),
            ("DELETE", r#"{"expectedRevision":2}"#, true, StatusCode::OK),
            ("GET", "", false, StatusCode::UNAUTHORIZED),
            ("PUT", body.as_str(), false, StatusCode::UNAUTHORIZED),
            (
                "PATCH",
                r#"{"expectedRevision":2}"#,
                false,
                StatusCode::UNAUTHORIZED,
            ),
            (
                "DELETE",
                r#"{"expectedRevision":2}"#,
                false,
                StatusCode::UNAUTHORIZED,
            ),
            ("PUT", r#"{}"#, true, StatusCode::BAD_REQUEST),
            (
                "PATCH",
                r#"{"expectedRevision":0}"#,
                true,
                StatusCode::BAD_REQUEST,
            ),
            ("DELETE", "", true, StatusCode::BAD_REQUEST),
            (
                "PUT",
                r#"{"expectedRevision":9223372036854775808,"partyLocationId":"invalid"}"#,
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "PATCH",
                r#"{"expectedRevision":9223372036854775808}"#,
                true,
                StatusCode::BAD_REQUEST,
            ),
            (
                "DELETE",
                r#"{"expectedRevision":9223372036854775808}"#,
                true,
                StatusCode::BAD_REQUEST,
            ),
        ];
        for (method, body, authenticated, status) in cases {
            let mut request = Request::builder()
                .method(method)
                .uri(&path)
                .header("content-type", "application/json");
            if authenticated {
                request = request.header("authorization", "Bearer test-token");
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::from(body.to_owned())).unwrap())
                .await
                .unwrap();
            assert_eq!(status, response.status(), "{method}");
            assert_eq!(
                "no-store",
                response.headers().get(header::CACHE_CONTROL).unwrap()
            );
            if method == "PUT" && status == StatusCode::OK {
                let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(
                    serde_json::json!(location),
                    value["assignment"]["partyLocationId"]
                );
                assert_eq!(
                    "Private evidence",
                    value["assignment"]["businessAssociation"]["reference"]
                );
                assert!(value["assignment"].get("advertisedInventory").is_none());
                assert!(value["assignment"].get("geography").is_none());
                assert!(value.get("sellerPartyId").is_none());
            }
        }
        assert_eq!(
            vec!["get", "assign", "change", "clear"],
            *capture.calls.lock().unwrap()
        );
    }
}
#[test]
fn errors_have_safe_actionable_statuses_and_never_expose_causes() {
    for (error, status) in [
        (LocationDefaultError::Forbidden, StatusCode::FORBIDDEN),
        (LocationDefaultError::SourceNotFound, StatusCode::NOT_FOUND),
        (
            LocationDefaultError::LocationNotFound,
            StatusCode::NOT_FOUND,
        ),
        (LocationDefaultError::LocationRetired, StatusCode::CONFLICT),
        (
            LocationDefaultError::DefaultNotAssigned,
            StatusCode::CONFLICT,
        ),
        (
            LocationDefaultError::ConcurrencyConflict,
            StatusCode::CONFLICT,
        ),
        (
            LocationDefaultError::RevisionExhausted,
            StatusCode::CONFLICT,
        ),
        (
            LocationDefaultError::InvalidRevision,
            StatusCode::BAD_REQUEST,
        ),
        (
            LocationDefaultError::TemporarilyUnavailable {
                source: application::error::static_error("Sensitive cause"),
            },
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            LocationDefaultError::InvalidPersistedState {
                source: application::error::static_error("Sensitive cause"),
            },
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let error = ApiError::from(error);
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("Sensitive cause")
        );
        assert_eq!(status, error.into_response().status());
    }
}
