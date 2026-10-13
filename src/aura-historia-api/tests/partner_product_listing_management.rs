use application::{
    operation_context::CredentialCapability,
    transaction::{Transaction, UnitOfWork},
};
use aura_historia_api::{
    app,
    auth::{AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal},
    state::{AppState, PartnerProductListingManagementState},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::{
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::{ProductListingId, ProductListingKey},
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
        ProductListingRepositoryError, ProductListingRepositoryFactory,
        ProductListingStorageVersion, ProductListingWriteEffects, VersionedProductListing,
        stamp_product_listing_event,
    },
    use_cases::{
        commands::partner_product_listings::PartnerProductListingMutationHandler,
        queries::partner_product_listings::{
            GetPartnerProductListingHandler, ListPartnerProductListingsHandler,
        },
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tokio::sync::Barrier;
use tower::ServiceExt;
use user_core::user_id::UserId;

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
            "partner" => (self.user, AuthMethod::CognitoJwt, BTreeSet::new()),
            "admin" => (self.admin, AuthMethod::CognitoJwt, BTreeSet::new()),
            "scoped-admin" => (
                self.admin,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([
                    CredentialCapability::ProductListingsRead,
                    CredentialCapability::ProductListingsWrite,
                ]),
            ),
            "unscoped-partner" => (self.user, AuthMethod::AuraAccessToken, BTreeSet::new()),
            "read-partner" => (
                self.user,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([CredentialCapability::ProductListingsRead]),
            ),
            "write-partner" => (
                self.user,
                AuthMethod::AuraAccessToken,
                BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            ),
            "scoped-partner" => (
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
struct Fixture {
    app: axum::Router,
    id: ProductListingId,
    source: ListingSourceId,
    other_source: ListingSourceId,
    other_id: ProductListingId,
    pool: sqlx::PgPool,
    authenticator: Arc<Auth>,
}
impl Fixture {
    fn collection(&self) -> String {
        format!("/api/v1/listing-sources/{}/product-listings", self.source)
    }
    fn detail(&self) -> String {
        format!("{}/{}", self.collection(), self.id)
    }
    fn target(&self, method: &str, id: ProductListingId, version: i64) -> String {
        let path = format!("{}/{}", self.collection(), id);
        match method {
            "DELETE" => format!("{path}?expectedVersion={version}"),
            "POST" => format!("{path}/restore"),
            _ => path,
        }
    }
    async fn current_version(&self) -> i64 {
        version(&self.pool, self.id).await
    }
}
async fn events(pool: &sqlx::PgPool, id: ProductListingId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn fixture() -> Fixture {
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
    sqlx::query("INSERT INTO parties (party_id,party_slug_id,name) VALUES ($1,$2,'Partner test')")
        .bind(party)
        .bind(format!("party-{party}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources (listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES ($1,$2,'Partner source',$3)").bind(source.as_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(&pool).await.unwrap();
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
    let other_source = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources (listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES ($1,$2,'Other source',$3)").bind(other_source.as_uuid()).bind(format!("source-{}",other_source.as_uuid())).bind(party).execute(&pool).await.unwrap();
    let id = seed_listing(&pool, source, "original-key").await;
    let other_id = seed_listing(&pool, other_source, "original-key").await;
    let reader = SqlxAdminProductListingReader::new(pool.clone());
    let authorization = || partnership_postgres::SqlxListingSourceAuthorization::new(pool.clone());
    let mutations = Arc::new(PartnerProductListingMutationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        authorization(),
        auction_postgres::SqlxAuctionReferenceValidatorFactory,
    ));
    let authenticator = Arc::new(Auth {
        admin: ids[0],
        user: ids[1],
    });
    let state = PartnerProductListingManagementState::new(
        Arc::new(GetPartnerProductListingHandler::new(
            reader.clone(),
            authorization(),
        )),
        Arc::new(ListPartnerProductListingsHandler::new(
            reader,
            authorization(),
        )),
        mutations.clone(),
        mutations.clone(),
        mutations,
        authenticator.clone(),
    );
    Fixture {
        app: app(AppState::new().with_partner_product_listing_management(state)),
        id,
        source,
        other_source,
        other_id,
        pool,
        authenticator,
    }
}
async fn seed_listing(pool: &sqlx::PgPool, source: ListingSourceId, key: &str) -> ProductListingId {
    let id = ProductListingId::new();
    let mut listing = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::from_title_and_suffix(
            &format!("Original title {}", id.as_uuid()),
            "a1b2c3",
        )
        .unwrap(),
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from(key).unwrap(),
        title: Some(localization::Localized::new(
            localization::Language::En,
            product_listing_core::title::Title::from("Original title"),
        )),
        description: Some(localization::Localized::new(
            localization::Language::De,
            product_listing_core::description::Description::from("Original description"),
        )),
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
    id
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
    if status.is_client_error() || status.is_server_error() {
        assert_eq!(
            "application/problem+json",
            response.headers().get("content-type").unwrap()
        );
    } else if status != StatusCode::NO_CONTENT {
        assert_eq!(
            "application/json",
            response.headers().get("content-type").unwrap()
        );
    }
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

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn every_endpoint_requires_source_grant_and_matching_delegated_capability() {
    let f = fixture().await;
    for token in [
        None,
        Some("invalid"),
        Some("admin"),
        Some("scoped-admin"),
        Some("unscoped-partner"),
        Some("partner"),
        Some("scoped-partner"),
        Some("read-partner"),
        Some("write-partner"),
    ] {
        for (method, directory, success) in [
            ("GET", true, StatusCode::OK),
            ("GET", false, StatusCode::OK),
            ("PATCH", false, StatusCode::OK),
            ("POST", false, StatusCode::OK),
            ("DELETE", false, StatusCode::NO_CONTENT),
        ] {
            let v = f.current_version().await;
            let target = if directory {
                f.collection()
            } else {
                f.target(method, f.id, v)
            };
            let (status, body) =
                request(&f.app, method, &target, token, json!({"expectedVersion":v})).await;
            let expected = match token {
                None | Some("invalid") => StatusCode::UNAUTHORIZED,
                Some("admin" | "scoped-admin" | "unscoped-partner") => StatusCode::FORBIDDEN,
                Some("read-partner") if method != "GET" => StatusCode::FORBIDDEN,
                Some("write-partner") if method == "GET" => StatusCode::FORBIDDEN,
                _ => success,
            };
            assert_eq!(expected, status, "{token:?} {method} {target}: {body}");
            if status == StatusCode::NO_CONTENT {
                assert_eq!(
                    StatusCode::OK,
                    request(
                        &f.app,
                        "POST",
                        &f.target("POST", f.id, v + 1),
                        Some("partner"),
                        json!({"expectedVersion":v+1})
                    )
                    .await
                    .0
                );
            }
        }
    }
    let other = format!(
        "/api/v1/listing-sources/{}/product-listings",
        f.other_source
    );
    for (method, directory) in [
        ("GET", true),
        ("GET", false),
        ("PATCH", false),
        ("DELETE", false),
        ("POST", false),
    ] {
        let target = if directory {
            other.clone()
        } else {
            format!(
                "{}/{}{}",
                other,
                f.other_id,
                if method == "DELETE" {
                    "?expectedVersion=1"
                } else if method == "POST" {
                    "/restore"
                } else {
                    ""
                }
            )
        };
        assert_eq!(
            StatusCode::FORBIDDEN,
            request(
                &f.app,
                method,
                &target,
                Some("scoped-partner"),
                json!({"expectedVersion":1})
            )
            .await
            .0
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn cross_source_ids_are_indistinguishable_from_missing_listings() {
    let f = fixture().await;
    let before = f.current_version().await;
    for method in ["GET", "PATCH", "DELETE", "POST"] {
        // Even an incorrect version must not reveal a cross-source target.
        let (status, foreign) = request(
            &f.app,
            method,
            &f.target(method, f.other_id, 999),
            Some("partner"),
            if method == "POST" {
                json!({"expectedVersion":999})
            } else {
                json!({"expectedVersion":999,"price":null})
            },
        )
        .await;
        let (missing_status, missing) = request(
            &f.app,
            method,
            &f.target(method, ProductListingId::new(), 999),
            Some("partner"),
            if method == "POST" {
                json!({"expectedVersion":999})
            } else {
                json!({"expectedVersion":999,"price":null})
            },
        )
        .await;
        assert_eq!(StatusCode::NOT_FOUND, status, "{method}: {foreign}");
        assert_eq!(status, missing_status);
        assert_eq!(foreign, missing);
        assert_eq!("PRODUCT_LISTING_NOT_FOUND", foreign["error"]);
    }
    assert_eq!(before, f.current_version().await);
    assert_eq!(1, version(&f.pool, f.other_id).await);
    assert_eq!(1, events(&f.pool, f.other_id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn invalid_foreign_state_is_not_exposed_but_invalid_owned_state_fails_closed() {
    let f = fixture().await;
    for id in [f.id, f.other_id] {
        sqlx::query(
            "UPDATE product_listings SET url = 'invalid URL' WHERE product_listing_id = $1",
        )
        .bind(id.as_uuid())
        .execute(&f.pool)
        .await
        .unwrap();
    }
    for method in ["GET", "PATCH", "DELETE", "POST"] {
        for (id, expected_status, expected_error) in [
            (
                f.other_id,
                StatusCode::NOT_FOUND,
                "PRODUCT_LISTING_NOT_FOUND",
            ),
            (
                ProductListingId::new(),
                StatusCode::NOT_FOUND,
                "PRODUCT_LISTING_NOT_FOUND",
            ),
            (
                f.id,
                StatusCode::INTERNAL_SERVER_ERROR,
                "PRODUCT_LISTING_INTERNAL_ERROR",
            ),
        ] {
            let (status, body) = request(
                &f.app,
                method,
                &f.target(method, id, 1),
                Some("partner"),
                json!({"expectedVersion":1}),
            )
            .await;
            assert_eq!(expected_status, status, "{method}: {body}");
            assert_eq!(expected_error, body["error"]);
        }
    }
    for id in [f.id, f.other_id] {
        assert_eq!(1, version(&f.pool, id).await);
        assert_eq!(1, events(&f.pool, id).await);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn directory_reads_originals_both_lifecycles_and_scopes_filters_and_cursors() {
    let f = fixture().await;
    let withdrawn = seed_listing(&f.pool, f.source, "Withdrawn/key with spaces").await;
    let extra = seed_listing(&f.pool, f.source, "literal%_key").await;
    assert_eq!(
        StatusCode::NO_CONTENT,
        request(
            &f.app,
            "DELETE",
            &f.target("DELETE", withdrawn, 1),
            Some("partner"),
            Value::Null
        )
        .await
        .0
    );
    let (status, page) = request(
        &f.app,
        "GET",
        &f.collection(),
        Some("read-partner"),
        Value::Null,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(3, page["items"].as_array().unwrap().len());
    for item in page["items"].as_array().unwrap() {
        assert_eq!(f.source.to_string(), item["source"]["listingSourceId"]);
        assert_ne!(f.other_id.to_string(), item["productListingId"]);
        assert!(item["expectedVersion"].as_u64().unwrap() > 0);
        assert_eq!("Original title", item["title"]["text"]);
    }
    for (query, expected) in [
        ("lifecycle=ACTIVE", 2),
        ("lifecycle=WITHDRAWN", 1),
        ("availability=AVAILABLE", 2),
        ("sourceListingId=original-key", 1),
        ("query=%20ORIGINAL%20", 3),
        ("query=literal%25_", 1),
    ] {
        let (status, page) = request(
            &f.app,
            "GET",
            &format!("{}?{query}", f.collection()),
            Some("partner"),
            Value::Null,
        )
        .await;
        assert_eq!(StatusCode::OK, status, "{query}: {page}");
        assert_eq!(expected, page["items"].as_array().unwrap().len(), "{query}");
    }
    let (status, detail) = request(
        &f.app,
        "GET",
        &f.target("GET", withdrawn, 2),
        Some("partner"),
        Value::Null,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("WITHDRAWN", detail["lifecycle"]);
    assert_eq!(Value::Null, detail["availability"]);
    assert_eq!("Original description", detail["description"]["text"]);
    assert_eq!("de", detail["description"]["language"]);
    assert_eq!(
        "https://source.example.test/private.jpg",
        detail["images"][0]
    );
    for field in [
        "embedding",
        "translations",
        "assessment",
        "rawObservation",
        "eventId",
        "projectionVersion",
        "watchlist",
        "score",
        "display",
    ] {
        assert!(detail.get(field).is_none(), "{field}");
    }
    let (status, _) = request(
        &f.app,
        "POST",
        &f.target("POST", withdrawn, 2),
        Some("partner"),
        json!({"expectedVersion":2}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    let (_, restored) = request(
        &f.app,
        "GET",
        &f.target("GET", withdrawn, 3),
        Some("partner"),
        Value::Null,
    )
    .await;
    assert_eq!("ACTIVE", restored["lifecycle"]);
    assert_eq!(Value::Null, restored["availability"]);
    let mut seen = std::collections::HashSet::new();
    let mut target = format!("{}?size=1&sort=created&order=asc", f.collection());
    let mut first_cursor = None;
    loop {
        let (status, page) = request(&f.app, "GET", &target, Some("partner"), Value::Null).await;
        assert_eq!(StatusCode::OK, status);
        for item in page["items"].as_array().unwrap() {
            assert!(seen.insert(item["productListingId"].as_str().unwrap().to_owned()));
        }
        let Some(cursor) = page["searchAfter"].as_str() else {
            break;
        };
        first_cursor.get_or_insert_with(|| cursor.to_owned());
        target = format!(
            "{}?size=1&sort=created&order=asc&searchAfter={cursor}",
            f.collection()
        );
    }
    assert_eq!(
        std::collections::HashSet::from([
            f.id.to_string(),
            withdrawn.to_string(),
            extra.to_string()
        ]),
        seen
    );
    let cursor = first_cursor.unwrap();
    for query in [
        format!("sort=updated&order=asc&searchAfter={cursor}"),
        format!("sort=created&order=desc&searchAfter={cursor}"),
        format!("sort=created&order=asc&lifecycle=ACTIVE&searchAfter={cursor}"),
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "GET",
                &format!("{}?{query}", f.collection()),
                Some("partner"),
                Value::Null
            )
            .await
            .0
        );
    }
    // Give access to another source so a cursor-scope failure is tested after authorization.
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id,listing_source_id) SELECT partnership_id,$1 FROM partnership_members WHERE user_id=$2").bind(f.other_source.as_uuid()).bind(f.authenticator.user.as_uuid()).execute(&f.pool).await.unwrap();
    let other = format!(
        "/api/v1/listing-sources/{}/product-listings",
        f.other_source
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &f.app,
            "GET",
            &format!("{other}?sort=created&order=asc&searchAfter={cursor}"),
            Some("partner"),
            Value::Null
        )
        .await
        .0
    );
    assert_eq!(
        StatusCode::BAD_REQUEST,
        request(
            &f.app,
            "GET",
            &format!("{}?listingSourceId={}", f.collection(), f.other_source),
            Some("partner"),
            Value::Null
        )
        .await
        .0
    );
    // Unknown retained IDs are not created by explicit restore; an empty authorized source reads empty.
    let empty_source = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources (listing_source_id,listing_source_slug_id,name,operator_party_id) SELECT $1,$2,'Empty source',operator_party_id FROM listing_sources WHERE listing_source_id=$3").bind(empty_source.as_uuid()).bind(format!("empty-{}",empty_source.as_uuid())).bind(f.source.as_uuid()).execute(&f.pool).await.unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id,listing_source_id) SELECT partnership_id,$1 FROM partnership_members WHERE user_id=$2").bind(empty_source.as_uuid()).bind(f.authenticator.user.as_uuid()).execute(&f.pool).await.unwrap();
    let other = format!("/api/v1/listing-sources/{empty_source}/product-listings");
    let (status, page) = request(&f.app, "GET", &other, Some("partner"), Value::Null).await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(json!([]), page["items"]);
    assert!(page.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn versioned_mutations_preserve_noops_and_explicit_lifecycle_semantics() {
    let f = fixture().await;
    let (_, initial) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    for (method, body) in [
        ("PATCH", json!({"expectedVersion":1})),
        ("POST", json!({"expectedVersion":1})),
    ] {
        let (status, result) = request(
            &f.app,
            method,
            &f.target(method, f.id, 1),
            Some("write-partner"),
            body,
        )
        .await;
        assert_eq!(StatusCode::OK, status);
        assert_eq!(
            json!({"productListingId":f.id.to_string(),"expectedVersion":1,"outcome":"UNCHANGED"}),
            result
        );
    }
    assert_eq!(1, events(&f.pool, f.id).await);
    let (_, unchanged) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!(initial, unchanged);
    let (status, changed) = request(
        &f.app,
        "PATCH",
        &f.detail(),
        Some("partner"),
        json!({
            "expectedVersion": 1,
            "price": {"type": "MONETARY", "currency": "EUR", "amount": 100},
            "priceEstimateMin": {"currency": "EUR", "amount": 90},
            "priceEstimateMax": {"currency": "EUR", "amount": 110},
            "availability": "RESERVED",
            "url": "https://source.example.test/corrected",
            "images": [
                "https://source.example.test/second.jpg",
                "https://source.example.test/first.jpg"
            ],
            "auction": {
                "lotNumber": "opaque / lot",
                "cataloguePosition": 7,
                "timing": {
                    "biddingOpens": "2026-10-12T09:00:00Z",
                    "scheduledCloses": "2026-10-13T09:00:00Z"
                }
            }
        }),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{changed}");
    assert_eq!(2, changed["expectedVersion"]);
    assert_eq!("CHANGED", changed["outcome"]);
    assert_eq!(2, events(&f.pool, f.id).await);
    let (_, detail) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!(initial["title"], detail["title"]);
    assert_eq!(initial["description"], detail["description"]);
    assert_eq!(
        initial["productListingTitleSlugId"],
        detail["productListingTitleSlugId"]
    );
    assert_eq!(100, detail["price"]["amount"]);
    assert_eq!(90, detail["priceEstimateMin"]["amount"]);
    assert_eq!(110, detail["priceEstimateMax"]["amount"]);
    assert_eq!("RESERVED", detail["availability"]);
    assert_eq!("opaque / lot", detail["auction"]["lotNumber"]);
    assert_eq!(7, detail["auction"]["cataloguePosition"]);
    assert_eq!(
        "https://source.example.test/second.jpg",
        detail["images"][0]
    );
    let (status, result) = request(
        &f.app,
        "PATCH",
        &f.detail(),
        Some("partner"),
        json!({
            "expectedVersion": 2,
            "price": null,
            "priceEstimateMin": null,
            "priceEstimateMax": null,
            "availability": null,
            "images": [],
            "auction": {
                "lotNumber": null,
                "cataloguePosition": null,
                "timing": {"biddingOpens": null, "scheduledCloses": null}
            }
        }),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{result}");
    let (_, cleared) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    for field in [
        "price",
        "priceEstimateMin",
        "priceEstimateMax",
        "availability",
        "auction",
    ] {
        assert_eq!(Value::Null, cleared[field]);
    }
    assert_eq!(json!([]), cleared["images"]);
    assert_eq!(detail["url"], cleared["url"]);
    assert_eq!(
        StatusCode::NO_CONTENT,
        request(
            &f.app,
            "DELETE",
            &f.target("DELETE", f.id, 3),
            Some("partner"),
            Value::Null
        )
        .await
        .0
    );
    let (_, withdrawn) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!("WITHDRAWN", withdrawn["lifecycle"]);
    assert_eq!(4, withdrawn["expectedVersion"]);
    assert_eq!(
        StatusCode::NO_CONTENT,
        request(
            &f.app,
            "DELETE",
            &f.target("DELETE", f.id, 4),
            Some("partner"),
            Value::Null
        )
        .await
        .0
    );
    let (status, error) = request(
        &f.app,
        "PATCH",
        &f.detail(),
        Some("partner"),
        json!({"expectedVersion":4}),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("PRODUCT_LISTING_WITHDRAWN", error["error"]);
    for method in ["PATCH", "DELETE", "POST"] {
        let (status, error) = request(
            &f.app,
            method,
            &f.target(method, f.id, 3),
            Some("partner"),
            json!({"expectedVersion":3}),
        )
        .await;
        assert_eq!(StatusCode::CONFLICT, status);
        assert_eq!("PRODUCT_LISTING_CONCURRENCY_CONFLICT", error["error"]);
    }
    let (_, still_withdrawn) =
        request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!(withdrawn, still_withdrawn);
    assert_eq!(4, events(&f.pool, f.id).await);
    let (status, restored) = request(
        &f.app,
        "POST",
        &f.target("POST", f.id, 4),
        Some("partner"),
        json!({"expectedVersion":4}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(5, restored["expectedVersion"]);
    let (_, active) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!("ACTIVE", active["lifecycle"]);
    assert_eq!(Value::Null, active["availability"]);
    let (status, noop) = request(
        &f.app,
        "POST",
        &f.target("POST", f.id, 5),
        Some("partner"),
        json!({"expectedVersion":5}),
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!("UNCHANGED", noop["outcome"]);
    let (status, error) = request(
        &f.app,
        "POST",
        &f.target("POST", f.id, 4),
        Some("partner"),
        json!({"expectedVersion":4}),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("PRODUCT_LISTING_CONCURRENCY_CONFLICT", error["error"]);
    let (_, still_active) = request(&f.app, "GET", &f.detail(), Some("partner"), Value::Null).await;
    assert_eq!(active, still_active);
    assert_eq!(5, events(&f.pool, f.id).await);
    assert_eq!(5, f.current_version().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn rejects_malformed_ids_versions_fields_and_directory_input() {
    let f = fixture().await;
    for body in [
        json!({}),
        json!({"expectedVersion":0}),
        json!({"expectedVersion":null}),
        json!({"expectedVersion":u64::MAX}),
        json!({"expectedVersion":1,"title":null}),
        json!({"expectedVersion":1,"description":null}),
        json!({"expectedVersion":1,"lifecycle":"WITHDRAWN"}),
        json!({"expectedVersion":1,"sourceListingId":"different"}),
        json!({"expectedVersion":1,"saleObservation":null}),
        json!({"expectedVersion":1,"images":null}),
        json!({"expectedVersion":1,"url":null}),
        json!({"expectedVersion":1,"auction":null}),
        json!({"expectedVersion":1,"auction":{"timing":{"extra":true}}}),
        json!({"expectedVersion":1,"price":{"type":"ON_REQUEST","amount":1}}),
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(&f.app, "PATCH", &f.detail(), Some("partner"), body)
                .await
                .0
        );
    }
    for body in [
        json!({}),
        json!({"expectedVersion":0}),
        json!({"expectedVersion":u64::MAX}),
        json!({"expectedVersion":1,"price":null}),
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "POST",
                &f.target("POST", f.id, 1),
                Some("partner"),
                body
            )
            .await
            .0
        );
    }
    for query in [
        "",
        "expectedVersion=0",
        "expectedVersion=-1",
        "expectedVersion=no",
        "expectedVersion=9223372036854775808",
        "expectedVersion=1&extra=true",
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "DELETE",
                &format!("{}?{query}", f.detail()),
                Some("partner"),
                Value::Null
            )
            .await
            .0
        );
    }
    for query in [
        "lifecycle=UNKNOWN",
        "availability=SOLD",
        "sort=price",
        "order=random",
        "sourceListingId=%20",
        "query=%00",
        "searchAfter=bad",
        "extra=true",
        "auctionId=bad",
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "GET",
                &format!("{}?{query}", f.collection()),
                Some("partner"),
                Value::Null
            )
            .await
            .0
        );
    }
    for id in [
        "legacy-key".to_owned(),
        UserId::new().to_string(),
        f.source.as_uuid().to_string(),
    ] {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "GET",
                &format!("{}/{id}", f.collection()),
                Some("partner"),
                Value::Null
            )
            .await
            .0
        );
        assert_eq!(
            StatusCode::BAD_REQUEST,
            request(
                &f.app,
                "GET",
                &format!("/api/v1/listing-sources/{id}/product-listings"),
                Some("partner"),
                Value::Null
            )
            .await
            .0
        );
    }
    assert_eq!(1, f.current_version().await);
    assert_eq!(1, events(&f.pool, f.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn dissolved_or_operator_incompatible_partnership_grants_deny_every_route() {
    let f = fixture().await;
    for state in ["DISSOLVED", "ACTIVE"] {
        sqlx::query("UPDATE partnerships SET business_state=$1")
            .bind(state)
            .execute(&f.pool)
            .await
            .unwrap();
        if state == "ACTIVE" {
            let other_party = uuid::Uuid::now_v7();
            sqlx::query("INSERT INTO parties (party_id,party_slug_id,name) VALUES ($1,$2,'Another operator')").bind(other_party).bind(format!("party-{other_party}")).execute(&f.pool).await.unwrap();
            sqlx::query(
                "UPDATE listing_sources SET operator_party_id=$1 WHERE listing_source_id=$2",
            )
            .bind(other_party)
            .bind(f.source.as_uuid())
            .execute(&f.pool)
            .await
            .unwrap();
        }
        for (method, directory) in [
            ("GET", true),
            ("GET", false),
            ("PATCH", false),
            ("DELETE", false),
            ("POST", false),
        ] {
            let target = if directory {
                f.collection()
            } else {
                f.target(method, f.id, 1)
            };
            assert_eq!(
                StatusCode::FORBIDDEN,
                request(
                    &f.app,
                    method,
                    &target,
                    Some("partner"),
                    json!({"expectedVersion":1})
                )
                .await
                .0
            );
        }
    }
    assert_eq!(1, f.current_version().await);
}

struct RacingFactory {
    barrier: Arc<Barrier>,
}
struct RacingRepository<R> {
    inner: R,
    barrier: Arc<Barrier>,
}
impl ProductListingRepositoryFactory<SqlxTransaction> for RacingFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingRepository + 'tx {
        RacingRepository {
            inner: SqlxProductListingRepositoryFactory.in_transaction(tx),
            barrier: self.barrier.clone(),
        }
    }
}
#[async_trait::async_trait]
impl<R: ProductListingRepository> ProductListingRepository for RacingRepository<R> {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_id(id).await
    }
    async fn find_by_id_in_source(
        &mut self,
        source: listing_source_core::ListingSourceId,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_id_in_source(source, id).await
    }

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_key(key).await
    }
    async fn insert(
        &mut self,
        product: &ProductListing,
        event: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        self.inner.insert(product, event).await
    }
    async fn update(
        &mut self,
        product: &ProductListing,
        version: ProductListingStorageVersion,
        event: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        self.barrier.wait().await;
        self.inner.update(product, version, event, effects).await
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn independent_management_sessions_surface_cas_race_as_conflict() {
    let f = fixture().await;
    let reader = SqlxAdminProductListingReader::new(f.pool.clone());
    let auth = || partnership_postgres::SqlxListingSourceAuthorization::new(f.pool.clone());
    let mutations = Arc::new(PartnerProductListingMutationHandler::new(
        SqlxUnitOfWork::new(f.pool.clone()),
        RacingFactory {
            barrier: Arc::new(Barrier::new(2)),
        },
        SqlxProductListingEventAppenderFactory,
        auth(),
        auction_postgres::SqlxAuctionReferenceValidatorFactory,
    ));
    let router = app(AppState::new().with_partner_product_listing_management(
        PartnerProductListingManagementState::new(
            Arc::new(GetPartnerProductListingHandler::new(reader.clone(), auth())),
            Arc::new(ListPartnerProductListingsHandler::new(reader, auth())),
            mutations.clone(),
            mutations.clone(),
            mutations,
            f.authenticator.clone(),
        ),
    ));
    let path = f.detail();
    let (left,right)=tokio::time::timeout(std::time::Duration::from_secs(15),async {
        tokio::join!(
            request(&router,"PATCH",&path,Some("partner"),json!({"expectedVersion":1,"price":{"type":"MONETARY","currency":"EUR","amount":100}})),
            request(&router,"PATCH",&path,Some("partner"),json!({"expectedVersion":1,"price":{"type":"MONETARY","currency":"EUR","amount":200}})),
        )
    }).await.unwrap();
    assert_eq!(
        1,
        usize::from(left.0 == StatusCode::OK) + usize::from(right.0 == StatusCode::OK)
    );
    let loser = if left.0 == StatusCode::OK {
        right
    } else {
        left
    };
    assert_eq!(StatusCode::CONFLICT, loser.0);
    assert_eq!("PRODUCT_LISTING_CONCURRENCY_CONFLICT", loser.1["error"]);
    assert_eq!(2, f.current_version().await);
    assert_eq!(2, events(&f.pool, f.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn auction_references_require_the_immutable_listing_source_and_directory_matches_membership()
{
    let f = fixture().await;
    let own = auction_core::AuctionId::new();
    let foreign = auction_core::AuctionId::new();
    for (auction, source) in [(own, f.source), (foreign, f.other_source)] {
        sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id) VALUES ($1,$2,'auction-key')").bind(auction.as_uuid()).bind(source.as_uuid()).execute(&f.pool).await.unwrap();
    }
    for (auction, status, code) in [
        (foreign, StatusCode::CONFLICT, "CONFLICT"),
        (
            auction_core::AuctionId::new(),
            StatusCode::NOT_FOUND,
            "AUCTION_NOT_FOUND",
        ),
    ] {
        let (actual, error) = request(
            &f.app,
            "PATCH",
            &f.detail(),
            Some("partner"),
            json!({"expectedVersion":1,"auction":{"auctionId":auction.to_string()}}),
        )
        .await;
        assert_eq!(status, actual);
        assert_eq!(code, error["error"]);
    }
    let (status, result) = request(
        &f.app,
        "PATCH",
        &f.detail(),
        Some("partner"),
        json!({
            "expectedVersion": 1,
            "auction": {
                "auctionId": own.to_string(),
                "lotNumber": "Lot 1",
                "cataloguePosition": 1
            }
        }),
    )
    .await;
    assert_eq!(StatusCode::OK, status, "{result}");
    let (status, page) = request(
        &f.app,
        "GET",
        &format!("{}?auctionId={own}", f.collection()),
        Some("partner"),
        Value::Null,
    )
    .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(1, page["size"]);
    assert_eq!(f.id.to_string(), page["items"][0]["productListingId"]);
    let (_, page) = request(
        &f.app,
        "GET",
        &format!("{}?auctionId={foreign}", f.collection()),
        Some("partner"),
        Value::Null,
    )
    .await;
    assert_eq!(json!([]), page["items"]);
    assert_eq!(2, f.current_version().await);
    assert_eq!(2, events(&f.pool, f.id).await);
}

#[derive(Clone, Copy)]
struct FaultyAuthorization {
    temporary: bool,
}
#[async_trait::async_trait]
impl partnership_service::ports::ListingSourceAuthorization for FaultyAuthorization {
    async fn can_write_source(
        &self,
        _: UserId,
        _: ListingSourceId,
    ) -> Result<bool, partnership_service::ports::SourceAuthorizationError> {
        let source = application::error::static_error("private authorization cause");
        if self.temporary {
            Err(
                partnership_service::ports::SourceAuthorizationError::TemporarilyUnavailable {
                    source,
                },
            )
        } else {
            Err(partnership_service::ports::SourceAuthorizationError::InvalidReadModel { source })
        }
    }
    async fn list_sources_user_administers(
        &self,
        _: UserId,
    ) -> Result<
        Vec<partnership_service::ports::AdministeredListingSource>,
        partnership_service::ports::SourceAuthorizationError,
    > {
        panic!("only path source authorization is needed")
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn authorization_dependencies_fail_closed_without_exposing_causes_or_touching_listings() {
    let f = fixture().await;
    for temporary in [true, false] {
        let authorization = FaultyAuthorization { temporary };
        let reader = SqlxAdminProductListingReader::new(f.pool.clone());
        let mutations = Arc::new(PartnerProductListingMutationHandler::new(
            SqlxUnitOfWork::new(f.pool.clone()),
            SqlxProductListingRepositoryFactory,
            SqlxProductListingEventAppenderFactory,
            authorization,
            auction_postgres::SqlxAuctionReferenceValidatorFactory,
        ));
        let router = app(AppState::new().with_partner_product_listing_management(
            PartnerProductListingManagementState::new(
                Arc::new(GetPartnerProductListingHandler::new(
                    reader.clone(),
                    authorization,
                )),
                Arc::new(ListPartnerProductListingsHandler::new(
                    reader,
                    authorization,
                )),
                mutations.clone(),
                mutations.clone(),
                mutations,
                f.authenticator.clone(),
            ),
        ));
        for (method, directory) in [
            ("GET", true),
            ("GET", false),
            ("PATCH", false),
            ("DELETE", false),
            ("POST", false),
        ] {
            let target = if directory {
                f.collection()
            } else {
                f.target(method, f.id, 1)
            };
            let (status, error) = request(
                &router,
                method,
                &target,
                Some("partner"),
                json!({"expectedVersion":1}),
            )
            .await;
            assert_eq!(
                if temporary {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                },
                status
            );
            assert_eq!(
                if temporary {
                    "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE"
                } else {
                    "PRODUCT_LISTING_INTERNAL_ERROR"
                },
                error["error"]
            );
            assert!(!error.to_string().contains("private authorization cause"));
        }
    }
    assert_eq!(1, f.current_version().await);
    assert_eq!(1, events(&f.pool, f.id).await);
}
