use super::support::*;
use super::{SCHEMA, aura_integration_test};
use axum::http::StatusCode;
use product_listing_service::use_cases::{
    CreateProductListingError, ProductListingIngestionCompletion, ProductListingIngestionError,
};
use serde_json::json;
use test_api::IntegrationTestService;

#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_recheck_capabilities_sources_and_memberships_without_treating_admission_as_authority()
 {
    let pool = test_api::get_postgres_client().await;
    let owner = actor(&pool, SCOPES).await;
    let ordinary = actor(&pool, SCOPES).await;
    let unscoped = actor(&pool, &[]).await;
    let source = source(&pool, &owner, "Granted source").await;
    let foreign_owner = actor(&pool, SCOPES).await;
    let foreign = super::support::source(&pool, &foreign_owner, "Other source").await;
    let server = Server::start(pool.clone()).await;
    for name in partner_names() {
        let missing = server.call(None, &name, json!({})).await;
        assert_eq!(StatusCode::UNAUTHORIZED, missing.status(), "{name}");
        assert!(
            missing.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("scope=")
        );
        let denied = server.call(Some(&unscoped.token), &name, json!({})).await;
        assert_eq!(StatusCode::FORBIDDEN, denied.status(), "{name}");
        assert!(
            denied.headers()["www-authenticate"]
                .to_str()
                .unwrap()
                .contains("insufficient_scope")
        );
    }
    assert_eq!(0, server.publisher.count());
    let client = server.client(Some(&owner.token), discover()).await;
    let consumer = server.client(Some(&ordinary.token), discover()).await;
    let tools = catalogue(&client).await;
    tool_error(
        &consumer,
        "listPartnerProductListings",
        json!({"listingSourceId":source.source}),
        "FORBIDDEN",
    )
    .await;
    tool_error(
        &client,
        "listPartnerProductListings",
        json!({"listingSourceId":foreign.source}),
        "FORBIDDEN",
    )
    .await;
    let forbidden = invoke(
        &consumer,
        &tools,
        "postPartnerProductListings",
        batch(source.source, vec![product("ordinary-sync")]),
    )
    .await;
    assert_eq!("FORBIDDEN", forbidden["failures"][0]["error"]);
    let queued = invoke(
        &consumer,
        &tools,
        "postAsyncPartnerProductListings",
        batch(source.source, vec![product("ordinary-async")]),
    )
    .await;
    assert_eq!(1, queued["acceptedCount"]);
    assert_create_forbidden(server.apply(server.publisher.last().remove(0)).await);
    let wrong = invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        batch(foreign.source, vec![product("wrong-source")]),
    )
    .await;
    assert_eq!(1, wrong["acceptedCount"]);
    assert_create_forbidden(server.apply(server.publisher.last().remove(0)).await);
    invoke(
        &client,
        &tools,
        "postPartnerProductListings",
        batch(source.source, vec![product("existing")]),
    )
    .await;
    sqlx::query("UPDATE access_tokens SET scopes=ARRAY['product-listings:read','listing-sources:read'] WHERE access_token_id=$1").bind(owner.token_id.as_uuid()).execute(&pool).await.unwrap();
    for name in [
        "postPartnerProductListings",
        "postAsyncPartnerProductListings",
    ] {
        assert_eq!(
            StatusCode::FORBIDDEN,
            server
                .call(
                    Some(&owner.token),
                    name,
                    batch(source.source, vec![product("no-capability")])
                )
                .await
                .status()
        );
    }
    invoke(
        &client,
        &tools,
        "listPartnerProductListings",
        json!({"listingSourceId":source.source}),
    )
    .await;
    sqlx::query("UPDATE access_tokens SET scopes=$2 WHERE access_token_id=$1")
        .bind(owner.token_id.as_uuid())
        .bind(SCOPES)
        .execute(&pool)
        .await
        .unwrap();
    invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        batch(source.source, vec![product("revoked-source")]),
    )
    .await;
    let pending = server.publisher.last().remove(0);
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE partnership_id=$1 AND listing_source_id=$2").bind(source.partnership).bind(source.source.as_uuid()).execute(&pool).await.unwrap();
    assert_create_forbidden(server.apply(pending).await);
    for name in [
        "listPartnerProductListings",
        "getShippingDefault",
        "getSourceLocationDefault",
    ] {
        tool_error(
            &client,
            name,
            json!({"listingSourceId":source.source}),
            "FORBIDDEN",
        )
        .await;
    }
    let page = invoke(&client, &tools, "getMyListingSources", json!({})).await;
    assert_eq!(json!([]), page["items"]);
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(source.partnership).bind(source.source.as_uuid()).execute(&pool).await.unwrap();
    invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        batch(source.source, vec![product("revoked-member")]),
    )
    .await;
    let pending = server.publisher.last().remove(0);
    sqlx::query("DELETE FROM partnership_members WHERE partnership_id=$1 AND user_id=$2")
        .bind(source.partnership)
        .bind(owner.user.as_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert_create_forbidden(server.apply(pending).await);
    tool_error(
        &client,
        "listPartyLocations",
        json!({"partyId":source.party}),
        "FORBIDDEN",
    )
    .await;
    tool_error(
        &client,
        "getProductListingDelivery",
        json!({"listingSourceId":source.source,"sourceListingId":"existing"}),
        "FORBIDDEN",
    )
    .await;
    // Even after revocation the scoped queue admission remains distinct from worker authorization.
    let admitted = invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        batch(source.source, vec![product("after-revocation")]),
    )
    .await;
    assert_eq!(1, admitted["acceptedCount"]);
    assert_create_forbidden(server.apply(server.publisher.last().remove(0)).await);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listings WHERE listing_source_id=$1")
            .bind(source.source.as_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(1, count);
    client.cancel().await.unwrap();
    consumer.cancel().await.unwrap();
}
fn assert_create_forbidden(
    result: Result<ProductListingIngestionCompletion, ProductListingIngestionError>,
) {
    assert!(
        matches!(
            result,
            Err(ProductListingIngestionError::Create(
                CreateProductListingError::Forbidden
            ))
        ),
        "{result:?}"
    );
}
