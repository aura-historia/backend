use application::operation_context::CredentialCapability;
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    shipping::ShippingState,
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
use shipping_postgres::{SqlxShippingReaderFactory, SqlxShippingRepositoryFactory};
use shipping_service::use_cases::*;
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
            "admin" | "operator" => [
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ListingSourcesWrite,
            ]
            .into(),
            "read" => [CredentialCapability::ListingSourcesRead].into(),
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id: self.0,
            auth_method: if token == "operator" {
                AuthMethod::CognitoJwt
            } else {
                AuthMethod::AuraAccessToken
            },
            capabilities,
        })
    }
}
fn applicability() -> Value {
    json!({"class":null,"variants":null,"quantity":null,"threshold":null})
}
fn advertised_content(amount: u64) -> Value {
    json!({"name":"Gallery advertised terms","attributedPartyId":null,"terms":{"schema":"SHIP_TERMS_V1","shipping":{"kind":"COVERAGE","coverage":{"reference_release":"iso-codes-4.20.1","completeness":"PARTIAL","applicability":applicability(),"exclusions":[],"dispatch_origin":null,"zones":[{"areas":[{"kind":"COUNTRY","code":"DE","reference_release":"iso-codes-4.20.1"}],"applicability":applicability(),"services":[{"code":"gallery-delivery","applicability":applicability(),"eligibility":"OFFERED","rate":{"price":{"kind":"FIXED","amount":{"amount_minor":amount,"currency":"EUR"}},"basis":"PER_ITEM","qualifying_scope":null,"components":{}},"lead_times":{"handling":null,"transit":null,"total":null},"features":[],"arranger":null,"performer":null}]}]}},"collection":{"applicability":applicability(),"dependency":"INDEPENDENT","eligibility":"OFFERED","site":null,"arrangement":{"kind":"BY_APPOINTMENT"}}}})
}
async fn request(
    router: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(t) = token {
        request = request.header("authorization", format!("Bearer {t}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!("no-store", response.headers().get("cache-control").unwrap());
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[aura_integration_test(services=[SCHEMA])]
async fn real_api_profile_commands_defaults_errors_and_protected_reads() {
    let p = get_postgres_client().await;
    let owner = PartyId::new();
    let source = ListingSourceId::new();
    let admin = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,'shipping-admin@example.test','FREE','ADMIN')").bind(admin.into_uuid()).execute(&p).await.unwrap();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,'api-shipping-gallery','Gallery')").bind(owner.into_uuid()).execute(&p).await.unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,'api-shipping-source','Gallery',$2)").bind(source.into_uuid()).bind(owner.into_uuid()).execute(&p).await.unwrap();
    let router = app(AppState::new().with_shipping(ShippingState {
        create: Arc::new(CreateShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        replace: Arc::new(ReplaceShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        retire: Arc::new(RetireShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        restore: Arc::new(RestoreShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        assign_default: Arc::new(AssignShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        clear_default: Arc::new(ClearShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingRepositoryFactory,
            SqlxSourceLocationAccessFactory,
        )),
        get: Arc::new(GetShippingProfileHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        list: Arc::new(ListShippingProfilesHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        get_default: Arc::new(GetShippingDefaultHandler::new(
            SqlxUnitOfWork::new(p.clone()),
            SqlxShippingReaderFactory,
            SqlxSourceLocationAccessFactory,
        )),
        authenticator: Arc::new(Auth(admin)),
    }));
    let path = format!("/api/v1/listing-sources/{source}/shipping-profiles");
    let default_path = format!("/api/v1/listing-sources/{source}/shipping-default");
    let body = json!({"creationKey":"gallery-profile","content":advertised_content(8000),"evidence":[{"reference":"https://example.test/protected-evidence","assertion":"ADVERTISED_TERMS"}]});
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        request(&router, "POST", &path, None, body.clone()).await.0
    );
    assert_eq!(
        StatusCode::FORBIDDEN,
        request(&router, "POST", &path, Some("read"), body.clone())
            .await
            .0
    );
    let mut unknown_party = body.clone();
    unknown_party["content"]["attributedPartyId"] = json!(PartyId::new());
    let (status, error) = request(&router, "POST", &path, Some("admin"), unknown_party).await;
    assert_eq!(StatusCode::NOT_FOUND, status);
    assert_eq!("PARTY_NOT_FOUND", error["error"]);
    let (status, original) = request(&router, "POST", &path, Some("admin"), body.clone()).await;
    assert_eq!(StatusCode::OK, status, "{original}");
    let id = original["shippingProfileId"].as_str().unwrap();
    assert!(id.starts_with("shp_"));
    assert!(!original.to_string().contains("protected-evidence"));
    assert_eq!(1, original["managementRevision"]);
    assert_eq!(
        original,
        request(&router, "POST", &path, Some("admin"), body.clone())
            .await
            .1
    );
    let detail = format!("{path}/{id}");
    assert_eq!(
        original,
        request(&router, "GET", &detail, Some("read"), Value::Null)
            .await
            .1
    );
    let (status, selected) = request(
        &router,
        "PUT",
        &default_path,
        Some("admin"),
        json!({"expectedRevision":1,"shippingProfileId":id}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(2, selected["revision"]);
    let replace = json!({"expectedRevision":1,"content":advertised_content(9500),"evidence":[]});
    let (status, updated) = request(&router, "PUT", &detail, Some("admin"), replace.clone()).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(2, updated["termsRevision"]);
    assert_eq!(2, updated["managementRevision"]);
    assert_eq!(
        selected,
        request(&router, "GET", &default_path, Some("read"), Value::Null)
            .await
            .1
    );
    let (status, error) = request(&router, "PUT", &detail, Some("admin"), replace).await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("SHIPPING_STALE_REVISION", error["error"]);
    let (status, retired) = request(
        &router,
        "DELETE",
        &detail,
        Some("admin"),
        json!({"expectedRevision":2}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("RETIRED", retired["lifecycle"]);
    assert_eq!(
        "RETIRED",
        request(&router, "GET", &default_path, Some("admin"), Value::Null)
            .await
            .1["eligibility"]
    );
    assert_eq!(
        StatusCode::CONFLICT,
        request(
            &router,
            "PUT",
            &default_path,
            Some("admin"),
            json!({"expectedRevision":2,"shippingProfileId":id})
        )
        .await
        .0
    );
    let (status, cleared) = request(
        &router,
        "DELETE",
        &default_path,
        Some("admin"),
        json!({"expectedRevision":2}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert!(cleared["shippingProfileId"].is_null());
    assert_eq!(3, cleared["revision"]);
    assert_eq!(
        StatusCode::OK,
        request(
            &router,
            "POST",
            &format!("{detail}/restore"),
            Some("admin"),
            json!({"expectedRevision":3})
        )
        .await
        .0
    );
    let (_, summaries) = request(
        &router,
        "GET",
        &format!("{path}?size=1"),
        Some("read"),
        Value::Null,
    )
    .await;
    assert_eq!(1, summaries.as_array().unwrap().len());
    assert!(summaries[0].get("content").is_none());
    assert!(summaries[0].get("evidence").is_none());
    let (_, empty) = request(
        &router,
        "GET",
        &format!("{path}?after={id}"),
        Some("read"),
        Value::Null,
    )
    .await;
    assert!(empty.as_array().unwrap().is_empty());
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &router,
            "GET",
            &format!("{path}?size=101"),
            Some("admin"),
            Value::Null
        )
        .await
        .0
    );
    let raw_path = format!(
        "/api/v1/listing-sources/{}/shipping-profiles",
        source.into_uuid()
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(&router, "GET", &raw_path, Some("admin"), Value::Null)
            .await
            .0
    );
    let mut missing = body.clone();
    missing["content"]["terms"]
        .as_object_mut()
        .unwrap()
        .remove("collection");
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(&router, "POST", &path, Some("admin"), missing)
            .await
            .0
    );

    let fixture: Value =
        serde_json::from_str(include_str!("../../../docs/fixtures/shipping-imports.json")).unwrap();
    let mut imported = fixture["create"].clone();
    let now = time::OffsetDateTime::now_utc();
    let stamp = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    imported["import"]["acquiredAt"] = json!(stamp(now));
    imported["import"]["observation"] = json!({"freshUntil":stamp(now+time::Duration::days(1)),"expiresAt":stamp(now+time::Duration::days(7))});
    let (status, created) = request(&router, "POST", &path, Some("admin"), imported.clone()).await;
    assert_eq!(StatusCode::OK, status, "{created}");
    let mut expected = fixture["committedProfile"].clone();
    expected["shippingProfileId"] = created["shippingProfileId"].clone();
    expected["listingSourceId"] = json!(source);
    assert_eq!(expected, created);
    assert_eq!(
        created,
        request(&router, "POST", &path, Some("admin"), imported.clone())
            .await
            .1
    );
    let identity = format!("{path}?objectKey=website-policy");
    let (status, recovered) = request(&router, "GET", &identity, Some("admin"), Value::Null).await;
    assert_eq!(StatusCode::OK, status, "{recovered}");
    assert_eq!(
        created["shippingProfileId"],
        recovered[0]["shippingProfileId"]
    );
    let mut conflicting = imported.clone();
    conflicting["content"]["name"] = json!("Changed request");
    let (status, error) = request(&router, "POST", &path, Some("admin"), conflicting).await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("SHIPPING_IMPORT_REQUEST_CONFLICT", error["error"]);
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'SHOPIFY')")
        .bind(source.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let (status, error) = request(&router, "POST", &path, Some("admin"), imported).await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("SHIPPING_IMPORT_AUTHORITY_STALE", error["error"]);
    // A delegated producer uses the existing configured provider directly.
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations(listing_source_id,domain) VALUES($1,'synthetic.myshopify.com')").bind(source.into_uuid()).execute(&p).await.unwrap();
    let mut provider = fixture["providerCreate"].clone();
    let now = time::OffsetDateTime::now_utc();
    provider["import"]["acquiredAt"] = json!(stamp(now));
    provider["import"]["observation"] = json!({"freshUntil":stamp(now+time::Duration::days(1)),"expiresAt":stamp(now+time::Duration::days(7))});
    let (status, provider) = request(&router, "POST", &path, Some("admin"), provider).await;
    assert_eq!(StatusCode::OK, status, "{provider}");
    let lookup = format!(
        "{path}?objectKey=provider-policy&integrationMethod=SHOPIFY&integrationDomain=synthetic.myshopify.com"
    );
    let (status, recovered) = request(&router, "GET", &lookup, Some("admin"), Value::Null).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(
        provider["shippingProfileId"],
        recovered[0]["shippingProfileId"]
    );
    sqlx::query("UPDATE shipping_profiles SET terms=jsonb_set(terms,'{schema}','\"SHIP_TERMS_FUTURE\"') WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&p).await.unwrap();
    let (status, error) = request(&router, "GET", &detail, Some("admin"), Value::Null).await;
    assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, status);
    assert_eq!("SHIPPING_STORAGE_ERROR", error["error"]);
    assert!(!error.to_string().contains("SHIP_TERMS_FUTURE"));
}
