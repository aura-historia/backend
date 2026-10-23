//! Production family composition and MCP authorization parity against isolated business state.
use aura_historia_mcp::{
    AuraTokenGate, Registry, TransportConfig, router,
    tools::{party_locations, source_location_defaults},
};
use aura_historia_runtime::Runtime;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use listing_source_core::ListingSourceId;
use oauth_core::resource::AuraResourcePolicy;
use party_core::party_id::PartyId;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};
const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestResult<T = ()> = Result<T, application::error::BoxError>;

fn app(runtime: &Runtime) -> Router {
    let mut operations = party_locations::operations(runtime.party_locations().clone()).unwrap();
    operations.extend(
        source_location_defaults::operations(runtime.source_location_defaults().clone()).unwrap(),
    );
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new(operations).unwrap(),
        Arc::new(AuraTokenGate::new(
            runtime.access_token_authentication().clone(),
            AuraResourcePolicy::production(),
        )),
    )
    .unwrap()
}
async fn call(
    app: &Router,
    token: &str,
    name: &str,
    args: Value,
) -> TestResult<(StatusCode, Value)> {
    let request=Request::post("/v1").header("host","mcp.example.test").header("content-type","application/json").header("accept","application/json,text/event-stream").header("mcp-protocol-version","2026-07-28").header("mcp-method","tools/call").header("mcp-name",name).header("authorization",format!("Bearer {token}")).body(Body::from(json!({
        "jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args,"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"location-fixture","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}
    }).to_string()))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024).await?;
    Ok((
        status,
        if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)?
        },
    ))
}
async fn success(app: &Router, token: &str, name: &str, args: Value) -> TestResult<Value> {
    let (status, v) = call(app, token, name, args).await?;
    assert_eq!(StatusCode::OK, status, "{v}");
    assert_ne!(true, v["result"]["isError"], "{v}");
    assert!(v.get("error").is_none(), "{v}");
    Ok(v["result"]["structuredContent"].clone())
}
async fn error(app: &Router, token: &str, name: &str, args: Value, code: &str) -> TestResult {
    let (status, v) = call(app, token, name, args).await?;
    assert_eq!(StatusCode::OK, status, "{v}");
    assert_eq!(true, v["result"]["isError"], "{v}");
    assert_eq!(code, v["result"]["content"][0]["text"], "{v}");
    Ok(())
}
async fn party(pool: &PgPool) -> TestResult<PartyId> {
    let id = PartyId::new();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Location fixture')",
    )
    .bind(id.as_uuid())
    .bind(format!("location-{}", id.as_uuid()))
    .execute(pool)
    .await?;
    Ok(id)
}
async fn source(pool: &PgPool, owner: PartyId) -> TestResult<ListingSourceId> {
    let id = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id,url) VALUES($1,$2,'Location source',$3,'https://shop.example.test')").bind(id.as_uuid()).bind(format!("location-{}", id.as_uuid())).bind(owner.as_uuid()).execute(pool).await?;
    Ok(id)
}
async fn user(pool: &PgPool) -> TestResult<UserId> {
    let id = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(id.as_uuid())
        .bind(format!("{id}@example.test"))
        .execute(pool)
        .await?;
    Ok(id)
}
async fn token(pool: &PgPool, user: UserId, scopes: &[&str]) -> TestResult<String> {
    let raw = RawAccessToken::new();
    let hash: HashedRawAccessToken = raw.clone().into();
    sqlx::query("INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) VALUES($1,$2,$3,$4,'Location fixture',$5,'USER')").bind(AccessTokenId::new().as_uuid()).bind(user.as_uuid()).bind(hash.short_token()).bind(hash.long_token_hash()).bind(scopes).execute(pool).await?;
    Ok(raw.into())
}
async fn grant(pool: &PgPool, party: PartyId, user: UserId) -> TestResult {
    sqlx::query("INSERT INTO party_location_management_grants(party_id,user_id,granted_actor) VALUES($1,$2,'fixture')").bind(party.as_uuid()).bind(user.as_uuid()).execute(pool).await?;
    Ok(())
}
async fn membership(
    pool: &PgPool,
    party: PartyId,
    user: UserId,
    source: ListingSourceId,
) -> TestResult {
    let id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(id)
        .bind(party.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(id)
        .bind(user.as_uuid())
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(id).bind(source.as_uuid()).execute(pool).await?;
    Ok(())
}
fn create_input(party: PartyId) -> Value {
    json!({"partyId":party,"label":"Store","roles":["BUSINESS_PREMISES","WAREHOUSE","REGISTERED_ADDRESS","CORRESPONDENCE"],"geography":{"referenceRelease":"iso-codes-4.20.1","addressText":"Private road 1","country":"DE","subdivision":"DE-BY","postalCode":"01234"},"position":{"latitudeDegrees":48.137,"longitudeDegrees":11.575,"precision":"PREMISES","accuracyMetres":1.5},"evidence":{"reference":"Private site observation","observedAt":"2026-10-20T12:00:00+02:00","scope":"SITE_DESCRIPTION"}})
}
const ALL_SCOPES: &[&str] = &[
    "parties:read",
    "parties:write",
    "listing-sources:read",
    "listing-sources:write",
];

#[aura_integration_test(services=[BUSINESS_SCHEMA],isolated=true)]
async fn should_preserve_full_location_payload_keyless_creation_paging_corrections_and_atomic_relocation()
 {
    preserve_full_location_payload_keyless_creation_paging_corrections_and_atomic_relocation()
        .await
        .unwrap();
}

async fn preserve_full_location_payload_keyless_creation_paging_corrections_and_atomic_relocation()
-> TestResult {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await?;
    let manager = user(&pool).await?;
    grant(&pool, owner, manager).await?;
    let token = token(&pool, manager, ALL_SCOPES).await?;
    let runtime = Runtime::from_pool(pool.clone());
    let app = app(&runtime);
    let first = success(&app, &token, "createPartyLocation", create_input(owner)).await?;
    let second = success(&app, &token, "createPartyLocation", create_input(owner)).await?;
    assert_ne!(first["id"], second["id"]);
    assert_eq!(4, first["roles"].as_array().unwrap().len());
    assert_eq!("01234", first["geography"]["postalCode"]);
    assert_eq!(1.5, first["position"]["accuracyMetres"]);
    assert_eq!("2026-10-20T10:00:00Z", first["evidence"]["observedAt"]);
    assert_eq!(1, first["inputRevision"]);
    let page = success(
        &app,
        &token,
        "listPartyLocations",
        json!({"partyId":owner,"limit":1}),
    )
    .await?;
    assert_eq!(first, page["items"][0]);
    assert_eq!(first["id"], page["next"]);
    let page = success(
        &app,
        &token,
        "listPartyLocations",
        json!({"partyId":owner,"limit":1,"after":page["next"]}),
    )
    .await?;
    assert_eq!(second, page["items"][0]);
    assert!(page["next"].is_null());
    let got = success(
        &app,
        &token,
        "getPartyLocation",
        json!({"partyId":owner,"locationId":first["id"]}),
    )
    .await?;
    assert_eq!(first, got);
    let correction = json!({"partyId":owner,"locationId":first["id"],"expectedRevision":1,"correction":"SAME_SITE","roles":[],"geography":null,"position":null,"evidence":null});
    let corrected = success(&app, &token, "correctPartyLocation", correction.clone()).await?;
    assert_eq!(2, corrected["revision"]);
    assert_eq!(2, corrected["inputRevision"]);
    assert_eq!("Store", corrected["label"]);
    assert_eq!(json!([]), corrected["roles"]);
    for field in ["geography", "position", "evidence"] {
        assert!(corrected.get(field).is_none());
    }
    error(&app, &token, "correctPartyLocation", correction, "CONFLICT").await?;
    let retired=success(&app,&token,"setPartyLocationLifecycle",json!({"partyId":owner,"locationId":second["id"],"expectedRevision":1,"lifecycle":"RETIRED"})).await?;
    assert_eq!("RETIRED", retired["lifecycle"]);
    assert_eq!(1, retired["inputRevision"]);
    let mut relocation = create_input(owner);
    relocation["relocates"] = json!({"id":first["id"],"revision":2});
    let replacement = success(&app, &token, "createPartyLocation", relocation.clone()).await?;
    assert_ne!(first["id"], replacement["id"]);
    error(&app, &token, "createPartyLocation", relocation, "CONFLICT").await?;
    let old = success(
        &app,
        &token,
        "getPartyLocation",
        json!({"partyId":owner,"locationId":first["id"]}),
    )
    .await?;
    assert_eq!("RETIRED", old["lifecycle"]);
    assert_eq!(3, old["revision"]);
    let page = success(&app, &token, "listPartyLocations", json!({"partyId":owner})).await?;
    assert_eq!(3, page["items"].as_array().unwrap().len());
    Ok(())
}

#[aura_integration_test(services=[BUSINESS_SCHEMA],isolated=true)]
async fn should_recheck_explicit_and_partnership_location_authority_and_delegated_scopes() {
    recheck_explicit_and_partnership_location_authority_and_delegated_scopes()
        .await
        .unwrap();
}

async fn recheck_explicit_and_partnership_location_authority_and_delegated_scopes() -> TestResult {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await?;
    let foreign = party(&pool).await?;
    let source = source(&pool, owner).await?;
    let explicit = user(&pool).await?;
    let member = user(&pool).await?;
    let outsider = user(&pool).await?;
    let full = token(&pool, explicit, ALL_SCOPES).await?;
    let member_token = token(&pool, member, ALL_SCOPES).await?;
    let outsider_token = token(&pool, outsider, ALL_SCOPES).await?;
    let read = token(&pool, explicit, &["parties:read"]).await?;
    let app = app(&Runtime::from_pool(pool.clone()));
    error(
        &app,
        &full,
        "createPartyLocation",
        create_input(owner),
        "FORBIDDEN",
    )
    .await?;
    grant(&pool, owner, explicit).await?;
    let site = success(&app, &full, "createPartyLocation", create_input(owner)).await?;
    let (status, _) = call(&app, &read, "createPartyLocation", create_input(owner)).await?;
    assert_eq!(StatusCode::FORBIDDEN, status);
    grant(&pool, foreign, explicit).await?;
    error(
        &app,
        &full,
        "getPartyLocation",
        json!({"partyId":foreign,"locationId":site["id"]}),
        "NOT_FOUND",
    )
    .await?;
    error(
        &app,
        &outsider_token,
        "listPartyLocations",
        json!({"partyId":owner}),
        "FORBIDDEN",
    )
    .await?;
    membership(&pool, owner, member, source).await?;
    success(
        &app,
        &member_token,
        "createPartyLocation",
        create_input(owner),
    )
    .await?;
    sqlx::query("DELETE FROM party_location_management_grants WHERE party_id=$1 AND user_id=$2")
        .bind(owner.as_uuid())
        .bind(explicit.as_uuid())
        .execute(&pool)
        .await?;
    error(
        &app,
        &full,
        "getPartyLocation",
        json!({"partyId":owner,"locationId":site["id"]}),
        "FORBIDDEN",
    )
    .await?;
    // Revoking an explicit reason leaves eligible partnership membership authoritative.
    grant(&pool, owner, member).await?;
    sqlx::query("DELETE FROM party_location_management_grants WHERE party_id=$1 AND user_id=$2")
        .bind(owner.as_uuid())
        .bind(member.as_uuid())
        .execute(&pool)
        .await?;
    success(
        &app,
        &member_token,
        "listPartyLocations",
        json!({"partyId":owner}),
    )
    .await?;
    sqlx::query("DELETE FROM partnership_members WHERE user_id=$1")
        .bind(member.as_uuid())
        .execute(&pool)
        .await?;
    error(
        &app,
        &member_token,
        "listPartyLocations",
        json!({"partyId":owner}),
        "FORBIDDEN",
    )
    .await?;
    Ok(())
}

#[aura_integration_test(services=[BUSINESS_SCHEMA],isolated=true)]
async fn should_preserve_source_default_evidence_eligibility_cas_clear_and_current_source_grants() {
    preserve_source_default_evidence_eligibility_cas_clear_and_current_source_grants()
        .await
        .unwrap();
}

async fn preserve_source_default_evidence_eligibility_cas_clear_and_current_source_grants()
-> TestResult {
    let pool = get_postgres_client().await;
    let owner = party(&pool).await?;
    let foreign_owner = party(&pool).await?;
    let source = source(&pool, owner).await?;
    let other = crate::source(&pool, owner).await?;
    let operator = user(&pool).await?;
    let manager = user(&pool).await?;
    grant(&pool, foreign_owner, manager).await?;
    membership(&pool, owner, operator, source).await?;
    let token = token(&pool, operator, ALL_SCOPES).await?;
    let manager_token = crate::token(&pool, manager, ALL_SCOPES).await?;
    let app = app(&Runtime::from_pool(pool.clone()));
    let site = success(&app, &token, "createPartyLocation", create_input(owner)).await?;
    let foreign_site = success(
        &app,
        &manager_token,
        "createPartyLocation",
        create_input(foreign_owner),
    )
    .await?;
    let empty = success(
        &app,
        &token,
        "getSourceLocationDefault",
        json!({"listingSourceId":source}),
    )
    .await?;
    assert_eq!(1, empty["revision"]);
    assert!(empty["assignment"].is_null());
    assert!(empty["eligibility"].is_null());
    error(
        &app,
        &token,
        "getSourceLocationDefault",
        json!({"listingSourceId":other}),
        "FORBIDDEN",
    )
    .await?;
    let mut assignment =
        json!({"listingSourceId":source,"expectedRevision":1,"partyLocationId":foreign_site["id"]});
    error(
        &app,
        &token,
        "assignSourceLocationDefault",
        assignment.clone(),
        "NOT_FOUND",
    )
    .await?;
    assignment["partyLocationId"] = site["id"].clone();
    assignment["businessAssociation"] =
        json!({"reference":"Private business evidence","scope":"ASSOCIATED_BUSINESS"});
    assignment["advertisedInventory"] =
        json!({"reference":"Private inventory evidence","scope":"ADVERTISED_INVENTORY"});
    let assigned = success(
        &app,
        &token,
        "assignSourceLocationDefault",
        assignment.clone(),
    )
    .await?;
    assert_eq!(2, assigned["revision"]);
    assert_eq!("ACTIVE", assigned["eligibility"]);
    assert_eq!(
        "Private business evidence",
        assigned["assignment"]["businessAssociation"]["reference"]
    );
    error(
        &app,
        &token,
        "assignSourceLocationDefault",
        assignment,
        "CONFLICT",
    )
    .await?;
    let changed = success(
        &app,
        &token,
        "changeSourceLocationDefaultApplicability",
        json!({"listingSourceId":source,"expectedRevision":2,"businessAssociation":null}),
    )
    .await?;
    assert_eq!(3, changed["revision"]);
    assert!(changed["assignment"].get("businessAssociation").is_none());
    assert_eq!(
        "Private inventory evidence",
        changed["assignment"]["advertisedInventory"]["reference"]
    );
    success(
        &app,
        &token,
        "setPartyLocationLifecycle",
        json!({"partyId":owner,"locationId":site["id"],"expectedRevision":1,"lifecycle":"RETIRED"}),
    )
    .await?;
    let retired = success(
        &app,
        &token,
        "getSourceLocationDefault",
        json!({"listingSourceId":source}),
    )
    .await?;
    assert_eq!("RETIRED", retired["eligibility"]);
    error(
        &app,
        &token,
        "changeSourceLocationDefaultApplicability",
        json!({"listingSourceId":source,"expectedRevision":3,"advertisedInventory":null}),
        "CONFLICT",
    )
    .await?;
    let cleared = success(
        &app,
        &token,
        "clearSourceLocationDefault",
        json!({"listingSourceId":source,"expectedRevision":3}),
    )
    .await?;
    assert_eq!(4, cleared["revision"]);
    assert!(cleared["assignment"].is_null());
    assert!(cleared["eligibility"].is_null());
    error(
        &app,
        &token,
        "clearSourceLocationDefault",
        json!({"listingSourceId":source,"expectedRevision":3}),
        "CONFLICT",
    )
    .await?;
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.as_uuid())
        .execute(&pool)
        .await?;
    error(
        &app,
        &token,
        "getSourceLocationDefault",
        json!({"listingSourceId":source}),
        "FORBIDDEN",
    )
    .await?;
    error(
        &app,
        &token,
        "clearSourceLocationDefault",
        json!({"listingSourceId":source,"expectedRevision":4}),
        "FORBIDDEN",
    )
    .await?;
    // Source-default grant loss does not remove the separate party membership authority.
    success(
        &app,
        &token,
        "getPartyLocation",
        json!({"partyId":owner,"locationId":site["id"]}),
    )
    .await?;
    Ok(())
}
