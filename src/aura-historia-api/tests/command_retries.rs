#[allow(dead_code)]
mod api_support;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, aura_integration_test, get_postgres_client,
};
use tower::ServiceExt;
use user_core::user_id::UserId;
const SCHEMA: Postgres = Postgres::new("migrations");
const SEARCH: OpenSearch = OpenSearch();
async fn command(
    app: &Router,
    user: UserId,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(
                    "Authorization",
                    format!(
                        "Bearer {}",
                        api_support::cognito_access_token_for_test_user(user)
                    ),
                )
                .header("Content-Type", "application/json")
                .header("Idempotency-Key", key)
                .body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN {
        assert_eq!(key, response.headers().get("idempotency-key").unwrap());
    }
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
async fn successful(
    app: &Router,
    user: UserId,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> Value {
    let (status, value) = command(app, user, method, path, key, body).await;
    assert!(status.is_success(), "{method} {path}: {status} {value}");
    value
}
#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn resource_commands_recover_original_results_after_later_edits_and_deletion() {
    let pool = get_postgres_client().await;
    let admin = api_support::seed_user("ADMIN").await;
    let app = api_support::aura_api_app().await;
    let create = json!({"name":"Retry Original Party"});
    let original = successful(
        &app,
        admin,
        "POST",
        "/api/v1/admin/parties",
        "party-create",
        create.clone(),
    )
    .await;
    assert_eq!(
        original,
        successful(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "party-create",
            create
        )
        .await
    );
    let party_path = format!(
        "/api/v1/admin/parties/{}",
        original["partyId"].as_str().unwrap()
    );
    let a = successful(
        &app,
        admin,
        "PATCH",
        &party_path,
        "party-edit-a",
        json!({"name":"Retry Party A"}),
    )
    .await;
    successful(
        &app,
        admin,
        "PATCH",
        &party_path,
        "party-edit-b",
        json!({"name":"Retry Party B"}),
    )
    .await;
    assert_eq!(
        a,
        successful(
            &app,
            admin,
            "PATCH",
            &party_path,
            "party-edit-a",
            json!({"name":"Retry Party A"})
        )
        .await
    );
    let current: String = sqlx::query_scalar("SELECT name FROM parties WHERE party_id=$1")
        .bind(
            original["partyId"]
                .as_str()
                .unwrap()
                .parse::<party_core::party_id::PartyId>()
                .unwrap()
                .into_uuid(),
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!("Retry Party B", current);
    let (status, problem) = command(
        &app,
        admin,
        "PATCH",
        &party_path,
        "party-edit-a",
        json!({"name":"changed intent"}),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("IDEMPOTENCY_MISMATCH", problem["error"]);
    successful(
        &app,
        admin,
        "DELETE",
        &party_path,
        "party-delete",
        Value::Null,
    )
    .await;
    successful(
        &app,
        admin,
        "DELETE",
        &party_path,
        "party-delete",
        Value::Null,
    )
    .await;
    // Administrator creation snapshots can recover the original identity without resurrecting a deleted row.
    assert_eq!(
        original,
        successful(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "party-create",
            json!({"name":"Retry Original Party"})
        )
        .await
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
    // Source creation excludes its generated proposed-party ID from submitted intent.
    let source = json!({"name":"Retry Source","operator":{"type":"NEW","name":"Retry Source Operator"},"ingestionConfiguration":[{"type":"PARTNER_API"}]});
    let created = successful(
        &app,
        admin,
        "POST",
        "/api/v1/admin/listing-sources",
        "source-create",
        source.clone(),
    )
    .await;
    assert_eq!(
        created,
        successful(
            &app,
            admin,
            "POST",
            "/api/v1/admin/listing-sources",
            "source-create",
            source
        )
        .await
    );
    let source_id = created["listingSourceId"].as_str().unwrap();
    let auction = json!({"listingSourceId":source_id,"sourceAuctionId":"retry-auction","name":{"text":"Original","language":"en"}});
    let created = successful(
        &app,
        admin,
        "POST",
        "/api/v1/admin/auctions",
        "auction-create",
        auction.clone(),
    )
    .await;
    assert_eq!(
        created,
        successful(
            &app,
            admin,
            "POST",
            "/api/v1/admin/auctions",
            "auction-create",
            auction
        )
        .await
    );
    let path = format!(
        "/api/v1/admin/auctions/{}",
        created["auctionId"].as_str().unwrap()
    );
    let patch = json!({"expectedVersion":1,"reportedLotCount":3});
    let updated = successful(&app, admin, "PATCH", &path, "auction-update", patch.clone()).await;
    assert_eq!(
        updated,
        successful(&app, admin, "PATCH", &path, "auction-update", patch.clone()).await
    );
    assert_eq!(
        StatusCode::CONFLICT,
        command(&app, admin, "PATCH", &path, "new-stale-action", patch)
            .await
            .0
    );
}
#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn profile_watchlist_decision_and_security_retries_do_not_reverse_later_actions() {
    let pool = get_postgres_client().await;
    let admin = api_support::seed_user("ADMIN").await;
    let user = api_support::seed_user("USER").await;
    let app = api_support::aura_api_app().await;
    let a = successful(
        &app,
        user,
        "PATCH",
        "/api/v1/me/account",
        "profile-a",
        json!({"firstName":"First"}),
    )
    .await;
    successful(
        &app,
        user,
        "PATCH",
        "/api/v1/me/account",
        "profile-b",
        json!({"firstName":"Later"}),
    )
    .await;
    assert_eq!(
        a,
        successful(
            &app,
            user,
            "PATCH",
            "/api/v1/me/account",
            "profile-a",
            json!({"firstName":"First"})
        )
        .await
    );
    let current: String = sqlx::query_scalar("SELECT first_name FROM users WHERE user_id=$1")
        .bind(user.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!("Later", current);
    let product = api_support::seed_product().await;
    let payload = json!({"productListingId":product,"notifications":true});
    let watch = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/watchlist",
        "watch-create",
        payload.clone(),
    )
    .await;
    let path = format!("/api/v1/me/watchlist/{product}");
    successful(
        &app,
        user,
        "PATCH",
        &path,
        "watch-edit",
        json!({"notifications":false}),
    )
    .await;
    assert_eq!(
        watch,
        successful(
            &app,
            user,
            "POST",
            "/api/v1/me/watchlist",
            "watch-create",
            payload
        )
        .await
    );
    successful(&app, user, "DELETE", &path, "watch-remove", Value::Null).await;
    successful(&app, user, "DELETE", &path, "watch-remove", Value::Null).await;
    let source =
        listing_source_core::ListingSourceId::try_from(api_support::seed_listing_source().await)
            .unwrap();
    let proposal = json!({"proposal":{"type":"EXISTING_LISTING_SOURCE","listingSourceId":source}});
    let submitted = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/partnership-applications",
        "application-submit",
        proposal.clone(),
    )
    .await;
    assert_eq!(
        submitted,
        successful(
            &app,
            user,
            "POST",
            "/api/v1/me/partnership-applications",
            "application-submit",
            proposal
        )
        .await
    );
    let id = submitted["id"].as_str().unwrap();
    let decision = format!("/api/v1/admin/partnership-applications/{id}/decision");
    successful(
        &app,
        admin,
        "PATCH",
        &format!("/api/v1/admin/partnership-applications/{id}"),
        "review-action",
        json!({}),
    )
    .await;
    let rejected = successful(
        &app,
        admin,
        "POST",
        &decision,
        "decision",
        json!({"decision":"REJECT"}),
    )
    .await;
    assert_eq!(
        rejected,
        successful(
            &app,
            admin,
            "POST",
            &decision,
            "decision",
            json!({"decision":"REJECT"})
        )
        .await
    );
    assert_eq!(
        "IDEMPOTENCY_MISMATCH",
        command(
            &app,
            admin,
            "POST",
            &decision,
            "decision",
            json!({"decision":"APPROVE"})
        )
        .await
        .1["error"]
    );
    let path = format!("/api/v1/admin/users/{user}/suspension");
    let suspended = successful(
        &app,
        admin,
        "PUT",
        &path,
        "suspend-a",
        json!({"reason":"support request"}),
    )
    .await;
    successful(&app, admin, "DELETE", &path, "unsuspend-b", Value::Null).await;
    assert_eq!(
        suspended,
        successful(
            &app,
            admin,
            "PUT",
            &path,
            "suspend-a",
            json!({"reason":"support request"})
        )
        .await
    );
    let suspended: bool = sqlx::query_scalar("SELECT suspended FROM users WHERE user_id=$1")
        .bind(user.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!suspended);
    // Category is bound across the shared administrator PATCH operation.
    let path = format!("/api/v1/admin/users/{user}");
    successful(
        &app,
        admin,
        "PATCH",
        &path,
        "category-key",
        json!({"tier":"PRO"}),
    )
    .await;
    assert_eq!(
        "IDEMPOTENCY_MISMATCH",
        command(
            &app,
            admin,
            "PATCH",
            &path,
            "category-key",
            json!({"role":"ADMIN"})
        )
        .await
        .1["error"]
    );
}
#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn issuance_never_replays_secrets_and_completed_billing_skips_the_existing_customer_guard() {
    let pool = get_postgres_client().await;
    let user = api_support::seed_user("USER").await;
    let app = api_support::aura_api_app().await;
    let issue = json!({"name":"retry-secret","scopes":["users:read"]});
    let issued = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/access-tokens",
        "token-issue",
        issue.clone(),
    )
    .await;
    assert!(issued["accessToken"].is_string());
    let (status, problem) = command(
        &app,
        user,
        "POST",
        "/api/v1/me/access-tokens",
        "token-issue",
        issue,
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("IDEMPOTENCY_RESULT_UNAVAILABLE", problem["error"]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens WHERE user_id=$1")
        .bind(user.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(1, count);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains(issued["accessTokenId"].as_str().unwrap())
    );
    let admin = api_support::seed_user("ADMIN").await;
    let client = json!({"client_name":"Retry client","tos_uri":"https://example.test/tos","policy_uri":"https://example.test/policy","client_uri":"https://example.test/client","logo_uri":"https://example.test/logo","redirect_uris":["https://example.test/callback"],"scope":["users:read"]});
    let client_issued = successful(
        &app,
        admin,
        "POST",
        "/api/v1/admin/oauth-clients",
        "oauth-issue",
        client.clone(),
    )
    .await;
    assert!(client_issued["client_secret"].is_string());
    let (status, problem) = command(
        &app,
        admin,
        "POST",
        "/api/v1/admin/oauth-clients",
        "oauth-issue",
        client,
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, status);
    assert_eq!("IDEMPOTENCY_RESULT_UNAVAILABLE", problem["error"]);
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains(client_issued["client_id"].as_str().unwrap())
    );
    let outcome:String=sqlx::query_scalar("SELECT outcome::text FROM application_command_receipts WHERE operation='CREATE_ACCESS_TOKEN'").fetch_one(&pool).await.unwrap();
    assert!(!outcome.contains(issued["accessToken"].as_str().unwrap()));
    // A terminal missing-customer decision must release owner exclusion for a later checkout.
    assert!(
        !command(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/portal",
            "portal-before-customer",
            Value::Null
        )
        .await
        .0
        .is_success()
    );
    let payload = json!({"plan":"PRO","cycle":"MONTHLY"});
    let checkout = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/billing/checkout",
        "checkout",
        payload.clone(),
    )
    .await;
    assert_eq!(
        checkout,
        successful(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/checkout",
            "checkout",
            payload.clone()
        )
        .await
    );
    assert_eq!(
        StatusCode::CONFLICT,
        command(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/checkout",
            "other-checkout",
            payload.clone()
        )
        .await
        .0
    );
    let manage = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/billing/manage",
        "manage-action",
        payload.clone(),
    )
    .await;
    assert_eq!(
        manage,
        successful(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/manage",
            "manage-action",
            payload.clone()
        )
        .await
    );
    let portal = successful(
        &app,
        user,
        "POST",
        "/api/v1/me/billing/portal",
        "portal-action",
        Value::Null,
    )
    .await;
    assert_eq!(
        portal,
        successful(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/portal",
            "portal-action",
            Value::Null
        )
        .await
    );
    sqlx::query("UPDATE external_command_operations SET state=jsonb_set(state,'{url_deadline}','0') WHERE operation='CREATE_BILLING_CHECKOUT'").execute(&pool).await.unwrap();
    assert_eq!(
        "IDEMPOTENCY_RESULT_UNAVAILABLE",
        command(
            &app,
            user,
            "POST",
            "/api/v1/me/billing/checkout",
            "checkout",
            payload
        )
        .await
        .1["error"]
    );
}

#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn every_opted_command_route_rejects_an_ambiguous_header_at_the_authenticated_boundary() {
    let admin = api_support::seed_user("ADMIN").await;
    let app = api_support::aura_api_app().await;
    let policies: Vec<Value> =
        serde_json::from_str(include_str!("../../../docs/command-retry-policies.json")).unwrap();
    for policy in policies {
        if !matches!(
            policy["policy"].as_str().unwrap(),
            "keyed" | "workflow" | "secret-fence" | "external-fence"
        ) {
            continue;
        }
        let pattern = policy["path"].as_str().unwrap();
        let path = pattern
            .split('/')
            .map(|p| {
                if !p.starts_with('{') {
                    return p.to_owned();
                }
                let p = p.to_ascii_lowercase();
                if p.contains("partnership_application") || p.contains("partnershipapplication") {
                    partnership_core::partnership_application_id::PartnershipApplicationId::new()
                        .to_string()
                } else if p.contains("partnership") {
                    partnership_core::partnership_id::PartnershipId::new().to_string()
                } else if p.contains("search_filter") || p.contains("searchfilter") {
                    search_filter_core::user_search_filter_id::UserSearchFilterId::new().to_string()
                } else if p.contains("product") {
                    product_listing_core::product_listing_id::ProductListingId::new().to_string()
                } else if p.contains("listing_source") || p.contains("listingsource") {
                    listing_source_core::ListingSourceId::new().to_string()
                } else if p.contains("location") {
                    party_core::party_location_id::PartyLocationId::new().to_string()
                } else if p.contains("party") {
                    party_core::party_id::PartyId::new().to_string()
                } else if p.contains("auction") {
                    auction_core::AuctionId::new().to_string()
                } else if p.contains("address") {
                    user_core::user_address::UserAddressId::new().to_string()
                } else if p.contains("client") {
                    credential_core::oauth_client_id::OAuthClientId::new().to_string()
                } else if p.contains("token") {
                    user_core::access_token::AccessTokenId::new().to_string()
                } else {
                    admin.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(policy["method"].as_str().unwrap())
                    .uri(&path)
                    .header(
                        "Authorization",
                        format!(
                            "Bearer {}",
                            api_support::cognito_access_token_for_test_user(admin)
                        ),
                    )
                    .header("Content-Type", "application/json")
                    .header("Idempotency-Key", "first,second")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{} {path} returned {status}: {e}", policy["method"]));
        assert_eq!(
            StatusCode::BAD_REQUEST,
            status,
            "{} {path}: {body}",
            policy["method"]
        );
        assert_eq!(
            "BAD_HEADER_VALUE", body["error"],
            "{} {path}: {body}",
            policy["method"]
        );
        assert_eq!("Idempotency-Key", body["source"]["field"]);
    }
}

#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn concurrent_handlers_cleanup_and_current_authority_keep_successful_fences() {
    use application::{
        idempotency::{CommandIntent, IdempotencyKey},
        operation_context::{OperationContext, Principal},
        transaction::{Transaction, UnitOfWork},
    };
    let pool = get_postgres_client().await;
    let admin = api_support::seed_user("ADMIN").await;
    let app = api_support::aura_api_app().await;
    let second = api_support::aura_api_app().await;
    let ctx = OperationContext {
        command_key: Some(IdempotencyKey::new("coordinate-key".into()).unwrap()),
        principal: Principal::User(admin),
        request_id: "coordinate".into(),
        correlation_id: "coordinate".into(),
    };
    let intent = CommandIntent::new(
        &ctx,
        "CREATE_PARTY",
        "",
        None,
        json!({"name":"Coordinated Party","phone":null,"email":null}),
    )
    .unwrap();
    let unit = platform_postgres::SqlxUnitOfWork::new(pool.clone());
    let mut active = unit.begin().await.unwrap();
    assert!(active.command_receipt(&intent).await.unwrap().is_none());
    assert_eq!(
        "IDEMPOTENCY_IN_PROGRESS",
        command(
            &second,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            json!({"name":"Coordinated Party"})
        )
        .await
        .1["error"]
    );
    // No successful evidence was recorded, so ending the first transaction admits the next attempt.
    active.commit().await.unwrap();
    let body = json!({"name":"Coordinated Party"});
    let (a, b) = tokio::join!(
        command(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            body.clone()
        ),
        command(
            &second,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            body.clone()
        )
    );
    assert!(a.0.is_success() || b.0.is_success());
    let original = successful(
        &app,
        admin,
        "POST",
        "/api/v1/admin/parties",
        "coordinate-key",
        body.clone(),
    )
    .await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM parties")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(1, count);
    sqlx::query("UPDATE application_command_receipts SET outcome_expires_at=clock_timestamp()-interval '1 second'").execute(&pool).await.unwrap();
    assert_eq!(
        "IDEMPOTENCY_RESULT_UNAVAILABLE",
        command(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            body.clone()
        )
        .await
        .1["error"]
    );
    let system = OperationContext {
        command_key: None,
        principal: Principal::System,
        request_id: "cleanup".into(),
        correlation_id: "cleanup".into(),
    };
    let cleanup = application::command_cleanup::CommandReceiptCleanupHandler(unit);
    assert_eq!((1, 0), cleanup.execute(&system, 1).await.unwrap());
    assert_eq!((0, 0), cleanup.execute(&system, 1).await.unwrap());
    assert_eq!(
        "IDEMPOTENCY_RESULT_UNAVAILABLE",
        command(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            body
        )
        .await
        .1["error"]
    );
    assert!(original["partyId"].is_string());
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(admin.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        StatusCode::UNAUTHORIZED,
        command(
            &app,
            admin,
            "POST",
            "/api/v1/admin/parties",
            "coordinate-key",
            json!({"name":"Coordinated Party"})
        )
        .await
        .0
    );
}

#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn interrupted_session_revocation_never_repeats_a_provider_action() {
    let pool = get_postgres_client().await;
    let admin = api_support::seed_user("ADMIN").await;
    let user = api_support::seed_user("USER").await;
    api_support::fail_session_revocation_for(user);
    let app = api_support::aura_api_app().await;
    let path = format!("/api/v1/admin/users/{user}/sessions/revoke");
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        command(&app, admin, "POST", &path, "revoke-action", Value::Null)
            .await
            .0
    );
    assert_eq!(
        "IDEMPOTENCY_IN_PROGRESS",
        command(&app, admin, "POST", &path, "revoke-action", Value::Null)
            .await
            .1["error"]
    );
    sqlx::query(
        "UPDATE external_command_operations SET lease_until=clock_timestamp()-interval '1 second'",
    )
    .execute(&pool)
    .await
    .unwrap();
    // The fake fails only once. A second provider execution would incorrectly return success here.
    let unavailable = command(&app, admin, "POST", &path, "revoke-action", Value::Null)
        .await
        .1;
    assert_eq!("IDEMPOTENCY_RESULT_UNAVAILABLE", unavailable["error"]);
    assert!(
        unavailable["detail"]
            .as_str()
            .unwrap()
            .contains("cannot be recovered or confirmed")
    );
    successful(
        &app,
        admin,
        "POST",
        &path,
        "explicit-new-revocation",
        Value::Null,
    )
    .await;
    successful(
        &app,
        admin,
        "POST",
        &path,
        "explicit-new-revocation",
        Value::Null,
    )
    .await;
}

#[aura_integration_test(services=[SCHEMA,SEARCH])]
async fn listing_aliases_recover_revisions_and_recheck_current_partnership_authority() {
    let pool = get_postgres_client().await;
    let admin = api_support::seed_user("ADMIN").await;
    let product = api_support::seed_product().await;
    let app = api_support::aura_api_app().await;
    let source: uuid::Uuid = sqlx::query_scalar(
        "SELECT listing_source_id FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(product.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    api_support::seed_operator_partnership_listing_source_grant(source).await;
    api_support::seed_partnership_membership(admin, source).await;
    let source = listing_source_core::ListingSourceId::try_from(source).unwrap();
    let config_path =
        format!("/api/v1/listing-sources/{source}/ingestion-configurations/woocommerce");
    let secret_a = "retry-credential-one";
    let secret_b = "retry-credential-two";
    let first = command(
        &app,
        admin,
        "PUT",
        &config_path,
        "config-action-a",
        json!({"webhookSecret":secret_a}),
    )
    .await;
    assert_eq!(StatusCode::CREATED, first.0);
    let later = command(
        &app,
        admin,
        "PUT",
        &config_path,
        "config-action-b",
        json!({"webhookSecret":secret_b}),
    )
    .await;
    assert_eq!(StatusCode::NO_CONTENT, later.0);
    assert_eq!(
        StatusCode::CREATED,
        command(
            &app,
            admin,
            "PUT",
            &config_path,
            "config-action-a",
            json!({"webhookSecret":secret_a})
        )
        .await
        .0
    );
    let current:String=sqlx::query_scalar("SELECT webhook_secret FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(secret_b, current);
    let snapshots:Vec<String>=sqlx::query_scalar("SELECT outcome::text FROM application_command_receipts WHERE operation='PUT_LISTING_SOURCE_CONFIGURATION'").fetch_all(&pool).await.unwrap();
    assert!(
        snapshots
            .iter()
            .all(|v| !v.contains(secret_a) && !v.contains(secret_b))
    );
    let admin_path = format!("/api/v1/admin/product-listings/{product}");
    let partner_path = format!("/api/v1/listing-sources/{source}/product-listings/{product}");
    let initial: i64 =
        sqlx::query_scalar("SELECT version FROM product_listings WHERE product_listing_id=$1")
            .bind(product.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let patch = json!({"expectedVersion":initial,"url":"https://example.test/correction"});
    let updated = successful(
        &app,
        admin,
        "PATCH",
        &admin_path,
        "listing-correction",
        patch.clone(),
    )
    .await;
    assert_eq!(
        updated,
        successful(
            &app,
            admin,
            "PATCH",
            &partner_path,
            "listing-correction",
            patch.clone()
        )
        .await
    );
    let version = initial + 1;
    successful(
        &app,
        admin,
        "DELETE",
        &format!("{admin_path}?expectedVersion={version}"),
        "listing-withdraw",
        Value::Null,
    )
    .await;
    assert_eq!(
        updated,
        successful(
            &app,
            admin,
            "PATCH",
            &partner_path,
            "listing-correction",
            patch.clone()
        )
        .await
    );
    successful(
        &app,
        admin,
        "POST",
        &format!("{admin_path}/restore"),
        "listing-restore",
        json!({"expectedVersion":version+1}),
    )
    .await;
    successful(
        &app,
        admin,
        "DELETE",
        &format!("{admin_path}?expectedVersion={version}"),
        "listing-withdraw",
        Value::Null,
    )
    .await;
    let current: (String, i64) = sqlx::query_as(
        "SELECT lifecycle,version FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(product.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(("ACTIVE".into(), version + 2), current);
    sqlx::query("DELETE FROM partnership_members WHERE user_id=$1")
        .bind(admin.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        StatusCode::FORBIDDEN,
        command(
            &app,
            admin,
            "PATCH",
            &partner_path,
            "listing-correction",
            patch.clone()
        )
        .await
        .0
    );
    assert_eq!(
        updated,
        successful(
            &app,
            admin,
            "PATCH",
            &admin_path,
            "listing-correction",
            patch
        )
        .await
    );
}
