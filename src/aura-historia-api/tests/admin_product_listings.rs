use application::{
    operation_context::CredentialCapability,
    transaction::{Transaction, UnitOfWork},
};
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AdminProductListingsState, AppState},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
};
use product_listing_postgres::{
    SqlxAdminProductListingReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::{
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, stamp_product_listing_event,
    },
    use_cases::{
        commands::admin_product_listings::AdminProductListingMutationHandler,
        queries::admin_product_listings::{
            GetAdminProductListingHandler, SearchAdminProductListingsHandler,
        },
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::user_id::UserId;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
struct Auth {
    admin: UserId,
    user: UserId,
}
#[async_trait::async_trait]
impl TokenAuthenticator for Auth {
    async fn authenticate(
        &self,
        token: &str,
        _: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let (user_id, auth_method, capabilities) = match token {
            "admin" => (self.admin, AuthMethod::CognitoJwt, BTreeSet::new()),
            "scoped-admin" => (
                self.admin,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]),
            ),
            "unscoped-admin" => (self.admin, AuthMethod::AuraAccessToken, BTreeSet::new()),
            "read-admin" => (
                self.admin,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([CredentialCapability::ProductListingsRead]),
            ),
            "write-admin" => (
                self.admin,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            ),
            "user" => (
                self.user,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]),
            ),
            _ => return Err(AuthError::InvalidCredentials),
        };
        Ok(TransportPrincipal::User {
            user_id,
            auth_method,
            capabilities,
        })
    }
}
async fn fixture() -> (axum::Router, ProductListingId, sqlx::PgPool) {
    let pool = get_postgres_client().await;
    let mut ids = Vec::new();
    for role in ["ADMIN", "USER"] {
        let id = UserId::new();
        sqlx::query("INSERT INTO users (user_id,email,role,tier) VALUES ($1,$2,$3,'FREE')")
            .bind(id.as_uuid())
            .bind(format!("{}@example.test", id.as_uuid()))
            .bind(role)
            .execute(&pool)
            .await
            .unwrap();
        ids.push(id);
    }
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id,party_slug_id,name) VALUES ($1,$2,'Admin test')")
        .bind(party)
        .bind(format!("party-{party}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources (listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES ($1,$2,'Admin source',$3)").bind(source.as_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(&pool).await.unwrap();
    // A Partnership grant does not make this user an administrator.
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1,$2)")
        .bind(partnership)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members (partnership_id,user_id) VALUES ($1,$2)")
        .bind(partnership)
        .bind(ids[1].as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id,listing_source_id) VALUES ($1,$2)").bind(partnership).bind(source.as_uuid()).execute(&pool).await.unwrap();
    let id = ProductListingId::new();
    let mut listing = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::from_title_and_suffix(
            &format!("Original title {}", id.as_uuid()),
            "a1b2c3",
        )
        .unwrap(),
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from("original-key").unwrap(),
        title: Some(localization::Localized::new(
            localization::Language::En,
            product_listing_core::title::Title::from("Original title"),
        )),
        description: None,
        pricing: ProductListingPricing::default(),
        availability: Some(
            product_listing_core::listing_availability::ListingAvailability::Available,
        ),
        url: url::Url::parse("https://source.example.test/listing").unwrap(),
        images: indexmap::IndexSet::from([
            product_listing_core::product_listing_image::ProductListingImage::new(
                url::Url::parse("https://source.example.test/private.jpg").unwrap(),
            ),
        ]),
        auction: None,
    })
    .unwrap();
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let event = stamp_product_listing_event(
        id,
        time::OffsetDateTime::now_utc(),
        listing.take_pending_event_payload().unwrap(),
    );
    SqlxProductListingRepositoryFactory
        .in_transaction(&mut tx)
        .insert(&listing, event.event_id)
        .await
        .unwrap();
    SqlxProductListingEventAppenderFactory
        .in_transaction(&mut tx)
        .append(&event)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let reader = SqlxAdminProductListingReader::new(pool.clone());
    let check = || {
        CheckUserAdminHandler::new(
            uow.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )
    };
    let mutations = Arc::new(AdminProductListingMutationHandler::new(
        uow.clone(),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        check(),
        auction_postgres::SqlxAuctionReferenceValidatorFactory,
    ));
    let state = AdminProductListingsState::new(
        Arc::new(GetAdminProductListingHandler::new(reader.clone(), check())),
        Arc::new(SearchAdminProductListingsHandler::new(reader, check())),
        mutations.clone(),
        mutations.clone(),
        mutations,
        Arc::new(Auth {
            admin: ids[0],
            user: ids[1],
        }),
    );
    (
        app(AppState::new().with_admin_product_listings(state)),
        id,
        pool,
    )
}
async fn request(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(
        "private, no-store",
        response.headers().get("cache-control").unwrap()
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}
async fn version(pool: &sqlx::PgPool, id: ProductListingId) -> i64 {
    sqlx::query_scalar("SELECT version FROM product_listings WHERE product_listing_id=$1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn all_admin_endpoints_enforce_persisted_role_and_delegated_capability_without_partner_grants()
 {
    let (app, id, pool) = fixture().await;
    let path = format!("/v1/admin/product-listings/{id}");
    for token in [
        None,
        Some("invalid"),
        Some("user"),
        Some("unscoped-admin"),
        Some("admin"),
        Some("scoped-admin"),
    ] {
        for (method, suffix, is_read, success) in [
            ("GET", "", true, StatusCode::OK),
            ("PATCH", "", false, StatusCode::OK),
            ("POST", "/restore", false, StatusCode::OK),
            ("DELETE", "", false, StatusCode::NO_CONTENT),
            ("GET", "directory", true, StatusCode::OK),
        ] {
            let expected_version = version(&pool, id).await;
            let target = if suffix == "directory" {
                "/v1/admin/product-listings".to_owned()
            } else if method == "DELETE" {
                format!("{path}?expectedVersion={expected_version}")
            } else {
                format!("{path}{suffix}")
            };
            let body = if method == "PATCH" || method == "POST" {
                json!({"expectedVersion":expected_version})
            } else {
                Value::Null
            };
            let (status, body) = request(&app, method, &target, token, body).await;
            let expected = match token {
                None | Some("invalid") => StatusCode::UNAUTHORIZED,
                Some("user") | Some("unscoped-admin") => StatusCode::FORBIDDEN,
                _ => success,
            };
            // The first successful deletion is retained; restore before an active-only PATCH in the next cycle.
            assert_eq!(expected, status, "{token:?} {method} {target}: {body}");
            if method == "DELETE" && status == StatusCode::NO_CONTENT {
                let current = version(&pool, id).await;
                let (status, _) = request(
                    &app,
                    "POST",
                    &format!("{path}/restore"),
                    token,
                    json!({"expectedVersion":current}),
                )
                .await;
                assert_eq!(StatusCode::OK, status);
            }
            let wrong_scope = if is_read { "write-admin" } else { "read-admin" };
            let (status, _) = request(
                &app,
                method,
                &target,
                Some(wrong_scope),
                if method == "PATCH" || method == "POST" {
                    json!({"expectedVersion":expected_version})
                } else {
                    Value::Null
                },
            )
            .await;
            assert_eq!(StatusCode::FORBIDDEN, status);
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn composed_admin_router_validates_fields_versions_lifecycle_and_privileged_images() {
    let (app, id, pool) = fixture().await;
    let path = format!("/v1/admin/product-listings/{id}");
    let (status, before) = request(&app, "GET", &path, Some("admin"), Value::Null).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("Original title", before["title"]["text"]);
    assert_eq!(
        "https://source.example.test/private.jpg",
        before["images"][0]
    );
    assert_eq!(1, before["expectedVersion"]);
    for body in [
        json!({}),
        json!({"expectedVersion":0}),
        json!({"expectedVersion":u64::MAX}),
        json!({"expectedVersion":1,"title":null}),
        json!({"expectedVersion":1,"images":null}),
        json!({"expectedVersion":1,"auction":{"timing":{"illegal":1}}}),
    ] {
        let (status, _) = request(&app, "PATCH", &path, Some("admin"), body).await;
        assert_eq!(StatusCode::BAD_REQUEST, status);
    }
    for invalid in ["not-an-id", &UserId::new().to_string()] {
        let (status, _) = request(
            &app,
            "GET",
            &format!("/v1/admin/product-listings/{invalid}"),
            Some("admin"),
            Value::Null,
        )
        .await;
        assert_eq!(StatusCode::BAD_REQUEST, status);
    }
    let (status, changed) = request(
        &app,
        "PATCH",
        &path,
        Some("admin"),
        json!({"expectedVersion":1,"price":{"type":"ON_REQUEST"},"availability":null,"images":[]}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("CHANGED", changed["outcome"]);
    assert_eq!(2, changed["expectedVersion"]);
    let (status, error) = request(
        &app,
        "PATCH",
        &path,
        Some("admin"),
        json!({"expectedVersion":1}),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("PRODUCT_LISTING_CONCURRENCY_CONFLICT", error["error"]);
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("{path}?expectedVersion=2"),
        Some("admin"),
        Value::Null,
    )
    .await;
    assert_eq!(StatusCode::NO_CONTENT, status);
    let (status, error) = request(
        &app,
        "PATCH",
        &path,
        Some("admin"),
        json!({"expectedVersion":3}),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("PRODUCT_LISTING_WITHDRAWN", error["error"]);
    let (status, detail) = request(&app, "GET", &path, Some("admin"), Value::Null).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("WITHDRAWN", detail["lifecycle"]);
    assert_eq!(before["title"], detail["title"]);
    let (status, page) = request(
        &app,
        "GET",
        "/v1/admin/product-listings?sourceListingId=original-key",
        Some("admin"),
        Value::Null,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("WITHDRAWN", page["items"][0]["lifecycle"]);
    let (status, restored) = request(
        &app,
        "POST",
        &format!("{path}/restore"),
        Some("admin"),
        json!({"expectedVersion":3}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(4, restored["expectedVersion"]);
    let (_, detail) = request(&app, "GET", &path, Some("admin"), Value::Null).await;
    assert_eq!(Value::Null, detail["availability"]);
    for method in ["GET", "PATCH", "DELETE", "POST"] {
        let missing = format!(
            "/v1/admin/product-listings/{}{}",
            ProductListingId::new(),
            if method == "DELETE" {
                "?expectedVersion=1"
            } else if method == "POST" {
                "/restore"
            } else {
                ""
            }
        );
        let (status, _) = request(
            &app,
            method,
            &missing,
            Some("admin"),
            json!({"expectedVersion":1}),
        )
        .await;
        assert_eq!(StatusCode::NOT_FOUND, status);
    }
    assert_eq!(4, version(&pool, id).await);
}
