use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    shipping::sync::ShippingSyncState,
    state::AppState,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use serde_json::{Value, json};
use shipping_postgres::SqlxShippingRepositoryFactory;
use shipping_sync_postgres::SqlxShippingSyncStoreFactory;
use shipping_sync_service::{canonical::CanonicalShippingSync, use_cases::ShippingSyncHandler};
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::user_id::UserId;
const SCHEMA: Postgres = Postgres::new("migrations");
struct Auth(UserId);
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let capabilities = match token {
            "manager" => [
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ListingSourcesWrite,
                CredentialCapability::ProductListingsRead,
                CredentialCapability::ProductListingsWrite,
            ]
            .into(),
            "write" => [
                CredentialCapability::ListingSourcesWrite,
                CredentialCapability::ProductListingsWrite,
            ]
            .into(),
            "source-only" => [
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ListingSourcesWrite,
            ]
            .into(),
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id: self.0,
            auth_method: AuthMethod::AuraAccessToken,
            capabilities,
        })
    }
}
async fn request(
    router: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("no-store", response.headers().get("cache-control").unwrap());
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[aura_integration_test(services=[SCHEMA])]
async fn api_receipts_capabilities_strict_bodies_dependencies_and_current_revocation() {
    let p = get_postgres_client().await;
    let owner = PartyId::new();
    let source = ListingSourceId::new();
    let admin = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,'shipping-sync-admin@example.test','FREE','ADMIN')").bind(admin.into_uuid()).execute(&p).await.unwrap();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,'sync-gallery','Gallery')",
    )
    .bind(owner.into_uuid())
    .execute(&p)
    .await
    .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,'sync-source','Gallery',$2)").bind(source.into_uuid()).bind(owner.into_uuid()).execute(&p).await.unwrap();
    let router = app(AppState::new().with_shipping_sync(ShippingSyncState {
        sync: Arc::new(ShippingSyncHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingSyncStoreFactory,
            SqlxSourceLocationAccessFactory,
            CanonicalShippingSync::new(
                SqlxShippingSyncStoreFactory,
                SqlxShippingRepositoryFactory,
                product_listing_postgres::SqlxProductListingRepositoryFactory,
                product_listing_postgres::SqlxProductListingEventAppenderFactory,
                product_listing_postgres::delivery::SqlxProductListingDeliveryReferencesFactory,
            ),
        )),
        authenticator: Arc::new(Auth(admin)),
    }));
    let base = format!("/api/v1/listing-sources/{source}/shipping-sync");
    let installation =
        json!({"schema":"SHIP_SYNC_V1","integration":{"kind":"SOURCE"},"expectedRevision":0});
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(
            &router,
            "PUT",
            &format!("{base}/installation"),
            None,
            installation.clone()
        )
        .await
        .0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(
            &router,
            "PUT",
            &format!("{base}/installation"),
            Some("source-only"),
            installation.clone()
        )
        .await
        .0
    );
    let (status, binding) = request(
        &router,
        "PUT",
        &format!("{base}/installation"),
        Some("manager"),
        installation,
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{binding}");
    let begin = json!({"schema":"SHIP_SYNC_V1","epoch":binding["epoch"],"key":"fetch-1","scope":"items","mode":"SNAPSHOT","expectedGeneration":0,"expectedSequence":0,"providerVersion":"website-v1","components":[false,false,true]});
    let (status, receipt) = request(
        &router,
        "POST",
        &format!("{base}/sessions"),
        Some("manager"),
        begin.clone(),
    )
    .await;
    assert_eq!(StatusCode::ACCEPTED, status, "{receipt}");
    assert_eq!("OPEN", receipt["application"]);
    assert_eq!(false, receipt["applied"]);
    assert_eq!("UNKNOWN", receipt["projection"]);
    let path = format!("{base}/sessions/{}", receipt["sessionId"].as_str().unwrap());
    assert_eq!(
        receipt["sessionId"],
        request(
            &router,
            "POST",
            &format!("{base}/sessions"),
            Some("manager"),
            begin
        )
        .await
        .1["sessionId"]
    );
    let now = time::OffsetDateTime::now_utc();
    let window = json!({"observedAt":now.format(&time::format_description::well_known::Rfc3339).unwrap(),"freshUntil":(now+time::Duration::hours(1)).format(&time::format_description::well_known::Rfc3339).unwrap(),"expiresAt":(now+time::Duration::hours(2)).format(&time::format_description::well_known::Rfc3339).unwrap()});
    let chunk = json!({"schema":"SHIP_SYNC_V1","records":[{"key":"missing","window":window,"objects":[],"assertion":{"kind":"ITEM","source_listing_id":"missing","selection":{"kind":"UNRESOLVED"},"rate_inputs":{"classes":null,"variants":null},"shipping":{"kind":"INHERIT"},"collection":{"kind":"INHERIT"}}}]});
    let (status, captured) = request(
        &router,
        "PUT",
        &format!("{path}/chunks/0"),
        Some("manager"),
        chunk.clone(),
    )
    .await;
    assert_eq!(StatusCode::ACCEPTED, status, "{captured}");
    assert_eq!("IN_PROGRESS", captured["rawCapture"]);
    assert_eq!(1, captured["chunks"].as_array().unwrap().len());
    let decoded = shipping_sync_json::decode_chunk::<shipping_sync_json::PublicReferences>(
        chunk.clone(),
        source,
    )
    .unwrap();
    let manifest = shipping_sync_json::manifest(&[decoded]);
    let manifest = json!({"schema":"SHIP_SYNC_V1","chunks":manifest.chunks,"records":manifest.records,"digest":shipping_sync_json::hex(&manifest.digest)});
    let (status, waiting) = request(
        &router,
        "POST",
        &format!("{path}/finalize"),
        Some("write"),
        manifest.clone(),
    )
    .await;
    assert_eq!(StatusCode::FAILED_DEPENDENCY, status, "{waiting}");
    assert_eq!("COMPLETE", waiting["rawCapture"]);
    assert_eq!("WAITING_DEPENDENCY", waiting["application"]);
    assert_eq!(false, waiting["applied"]);
    assert_eq!("LISTING_MISSING", waiting["failure"]);
    let mut malformed = chunk;
    malformed["records"][0]["credentials"] = json!("private-canary");
    let (status, error) = request(
        &router,
        "PUT",
        &format!("{path}/chunks/0"),
        Some("manager"),
        malformed,
    )
    .await;
    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert!(!error.to_string().contains("private-canary"));
    let pending = format!(
        "{base}/installations/{}/pending",
        binding["epoch"].as_str().unwrap()
    );
    let (_, pending) = request(&router, "GET", &pending, Some("manager"), Value::Null).await;
    assert_eq!(receipt["sessionId"], pending["sessionIds"][0]);
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(admin.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(
            &router,
            "POST",
            &format!("{path}/finalize"),
            Some("manager"),
            manifest
        )
        .await
        .0
    );
}
