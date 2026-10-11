use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, UserAddressesState},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use geo_postgres::SqlxResolutionRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use serde_json::{Value, json};
use std::sync::Arc;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{user_address::UserAddressId, user_id::UserId};
use user_postgres::SqlxAddressBookFactory;
use user_service::address_book::AddressBookHandler;
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
struct Auth {
    owner: UserId,
    foreign: UserId,
}
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let (id, capabilities) = match token {
            "owner" => (
                self.owner,
                [
                    CredentialCapability::UsersRead,
                    CredentialCapability::UsersWrite,
                ]
                .into(),
            ),
            "foreign" => (
                self.foreign,
                [
                    CredentialCapability::UsersRead,
                    CredentialCapability::UsersWrite,
                ]
                .into(),
            ),
            "readonly" => (self.owner, [CredentialCapability::UsersRead].into()),
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id: id,
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
    let mut r = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        r = r.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("no-store", response.headers().get("cache-control").unwrap());
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
#[aura_integration_test(services=[SCHEMA])]
async fn private_rest_contract_rejects_owner_input_and_exposes_fenced_preferences() {
    let pool = get_postgres_client().await;
    let owner = UserId::new();
    let foreign = UserId::new();
    for id in [owner, foreign] {
        sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
            .bind(id.into_uuid())
            .bind(format!("{id}@example.test"))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('PRIVATE_API_TEST',true)").execute(&pool).await.unwrap();
    let handler = Arc::new(AddressBookHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxAddressBookFactory,
        SqlxResolutionRepositoryFactory,
    ));
    let router = app(AppState::new().with_user_addresses(UserAddressesState {
        commands: handler.clone(),
        queries: handler,
        authenticator: Arc::new(Auth { owner, foreign }),
    }));
    let id = UserAddressId::new();
    let base = "/api/v1/me/addresses";
    let path = format!("{base}/{id}");
    let input = json!({"addressId":id,"label":"Home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Synthetic private home"}});
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, "POST", base, None, input.clone()).await.0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(&router, "POST", base, Some("readonly"), input.clone())
            .await
            .0
    );
    let mut malicious = input.clone();
    malicious["owner"] = json!(foreign);
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(&router, "POST", base, Some("owner"), malicious)
            .await
            .0
    );
    let (status, saved) = request(&router, "POST", base, Some("owner"), input.clone()).await;
    assert_eq!(StatusCode::CREATED, status);
    assert_eq!(json!(id), saved["addressId"]);
    assert_eq!(json!(false), saved["preferred"]);
    assert_eq!(
        StatusCode::CREATED,
        request(&router, "POST", base, Some("owner"), input).await.0
    );
    let (_, page) = request(&router, "GET", base, Some("owner"), json!({})).await;
    assert_eq!(1, page["items"].as_array().unwrap().len());
    assert_eq!(
        StatusCode::NOT_FOUND,
        request(&router, "GET", &path, Some("foreign"), json!({}))
            .await
            .0
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &router,
            "GET",
            &format!("{base}/{}", id.into_uuid()),
            Some("owner"),
            json!({})
        )
        .await
        .0
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &router,
            "GET",
            &format!("{base}?after=private-input"),
            Some("owner"),
            json!({})
        )
        .await
        .0
    );
    let destination = "/api/v1/me/browsing-destination";
    assert_eq!(
        StatusCode::NOT_FOUND,
        request(
            &router,
            "PUT",
            destination,
            Some("foreign"),
            json!({"expectedVersion":1,"destination":{"kind":"ADDRESS","addressId":id}})
        )
        .await
        .0
    );
    assert_eq!(StatusCode::BAD_REQUEST,request(&router,"PUT",destination,Some("owner"),json!({"expectedVersion":1,"destination":{"kind":"ADDRESS","addressId":id,"country":"DE"}})).await.0);
    let (_, preferred) = request(
        &router,
        "PUT",
        destination,
        Some("owner"),
        json!({"expectedVersion":1,"destination":{"kind":"ADDRESS","addressId":id}}),
    )
    .await;
    assert_eq!(json!(2), preferred["version"]);
    let (_, resolved) = request(
        &router,
        "POST",
        &format!("{destination}/resolve"),
        Some("owner"),
        json!({"destination":{"kind":"COUNTRY_POSTAL","country":"GB","postalCode":"SW1A 1AA"}}),
    )
    .await;
    assert_eq!(json!("GB"), resolved["submitted"]["country"]);
    assert!(resolved["derived"].is_null());
    let (_, resolution) = request(
        &router,
        "GET",
        &format!("{path}/resolution"),
        Some("owner"),
        json!({}),
    )
    .await;
    assert_eq!(json!("PENDING"), resolution["status"]);
    assert_eq!(json!(false), resolution["evidenceAvailable"]);
    let edit = json!({"expectedVersion":1,"label":"New home","geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Synthetic replacement"}});
    let (status, changed) = request(&router, "PUT", &path, Some("owner"), edit.clone()).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(json!(2), changed["inputRevision"]);
    assert_eq!(
        StatusCode::CONFLICT,
        request(&router, "PUT", &path, Some("owner"), edit).await.0
    );
    assert_eq!(
        StatusCode::NO_CONTENT,
        request(
            &router,
            "DELETE",
            &path,
            Some("owner"),
            json!({"expectedVersion":2})
        )
        .await
        .0
    );
    let (_, dest) = request(&router, "GET", destination, Some("owner"), json!({})).await;
    assert_eq!(json!("NONE"), dest["destination"]["kind"]);
    assert_eq!(json!(3), dest["version"]);
}
