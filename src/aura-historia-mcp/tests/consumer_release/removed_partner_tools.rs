use super::support::*;
use super::{SCHEMA, aura_integration_test};
use axum::http::StatusCode;
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use sqlx::PgPool;
use test_api::IntegrationTestService;

/// Partner operations removed from MCP (#2481); partners use the REST API instead.
const REMOVED: [&str; 37] = [
    "getMyListingSources",
    "listPartnerProductListings",
    "getPartnerProductListing",
    "updatePartnerProductListing",
    "withdrawPartnerProductListing",
    "restorePartnerProductListing",
    "postPartnerProductListings",
    "patchPartnerProductListings",
    "putPartnerProductListings",
    "deletePartnerProductListings",
    "postAsyncPartnerProductListings",
    "patchAsyncPartnerProductListings",
    "putAsyncPartnerProductListings",
    "deleteAsyncPartnerProductListings",
    "putShopifyListingSourceIngestionConfiguration",
    "createPartyLocation",
    "listPartyLocations",
    "getPartyLocation",
    "correctPartyLocation",
    "setPartyLocationLifecycle",
    "getSourceLocationDefault",
    "assignSourceLocationDefault",
    "changeSourceLocationDefaultApplicability",
    "clearSourceLocationDefault",
    "listShippingProfiles",
    "createShippingProfile",
    "getShippingProfile",
    "replaceShippingProfile",
    "retireShippingProfile",
    "restoreShippingProfile",
    "getShippingDefault",
    "assignShippingDefault",
    "clearShippingDefault",
    "getProductListingDelivery",
    "changeProductListingDelivery",
    "submitProductListingDelivery",
    // Never exposed: it accepts a plaintext webhook secret.
    "putWoocommerceListingSourceIngestionConfiguration",
];
const PARTNER_SCOPES: &[&str] = &[
    "users:read",
    "listing-sources:read",
    "listing-sources:write",
    "product-listings:read",
    "product-listings:write",
    "parties:read",
    "parties:write",
];

#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_reject_every_removed_partner_tool_as_unknown_after_admission_without_side_effects()
{
    let pool = test_api::get_postgres_client().await;
    let owner = actor(&pool, PARTNER_SCOPES).await;
    let source = granted_source(&pool, &owner).await;
    let server = Server::start(pool.clone()).await;
    let client = server.client(Some(&owner.token), discover()).await;
    let tools = catalogue(&client).await;
    let rows = business_rows(&pool).await;
    let arguments = json!({
        "listingSourceId": source,
        "products": [{"sourceListingId":"removed","title":{"language":"en","text":"Painting"},"url":"https://shop.example.test/painting","images":[]}],
        "configuration": {"webhookSecret":"PRIVATE_REMOVED_SECRET"},
    });
    for name in REMOVED {
        assert!(!tools.contains_key(name), "{name} is still listed");
        let response = server
            .call(Some(&owner.token), name, arguments.clone())
            .await;
        assert_eq!(StatusCode::OK, response.status(), "{name}");
        let body: Value = response.json().await.unwrap();
        assert_unknown_tool(name, &body);
        let anonymous = server.call(None, name, json!({})).await;
        assert_eq!(StatusCode::OK, anonymous.status(), "{name}");
        assert_unknown_tool(name, &anonymous.json().await.unwrap());
        let lambda = server
            .lambda(
                Some(&owner.token),
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
        assert_unknown_tool(name, &lambda);
        let sdk = client
            .call_tool(
                CallToolRequestParams::new(name.to_owned())
                    .with_arguments(serde_json::from_value(arguments.clone()).unwrap()),
            )
            .await;
        assert!(sdk.is_err(), "{name} was callable through the SDK client");
    }
    assert_eq!(rows, business_rows(&pool).await);
    client.cancel().await.unwrap();
}

fn assert_unknown_tool(name: &str, body: &Value) {
    assert_eq!(-32602, body["error"]["code"], "{name}: {body}");
    assert!(body.get("result").is_none(), "{name}: {body}");
    assert!(!body.to_string().contains("PRIVATE_REMOVED_SECRET"));
}

/// Total rows across the business schema; any partner write or admission record would change it.
async fn business_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM((xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', schemaname, tablename), false, true, '')))[1]::text::bigint), 0)::bigint FROM pg_tables WHERE schemaname = current_schema()",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A source the actor may manage through REST, so authority alone cannot explain a rejection.
async fn granted_source(pool: &PgPool, actor: &Actor) -> ListingSourceId {
    let party = PartyId::new();
    let source = ListingSourceId::new();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Removed tools party')",
    )
    .bind(party.as_uuid())
    .bind(format!("party-{}", party.as_uuid()))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id,url) VALUES($1,$2,'Removed tools source',$3,'https://shop.example.test')").bind(source.as_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(party.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(actor.user.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.as_uuid()).execute(pool).await.unwrap();
    source
}
