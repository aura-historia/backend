use super::*;
use aura_historia_runtime::Runtime;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
async fn seed_source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Operator')")
        .bind(party)
        .bind(format!("party-{party}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Partner source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
    (party, source)
}
async fn grant_source(
    pool: &sqlx::PgPool,
    user_id: user_core::user_id::UserId,
    party: uuid::Uuid,
    source: ListingSourceId,
) {
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user_id.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
}

#[aura_integration_test(services=[SCHEMA], isolated=true)]
async fn should_commit_items_independently_keep_duplicate_order_and_recheck_partner_authority() {
    let pool = get_postgres_client().await;
    let context = context();
    let Principal::DelegatedUser { user_id, .. } = context.operation.principal else {
        panic!("delegated actor")
    };
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let (party, source) = seed_source(&pool).await;
    grant_source(&pool, user_id, party, source).await;
    let runtime = Runtime::from_pool(pool.clone());
    let registry =
        Registry::new(operations(runtime.partner_product_listing_batches().clone()).unwrap())
            .unwrap();
    let product = |id: &str| json!({"sourceListingId":id,"title":{"language":"en","text":"Listing"},"url":"https://example.test/listing","images":[]});
    let created=call(&registry,NAMES[0],json!({"listingSourceId":source,"products":[product("same"),product("same"),product("after")]}),context.clone()).await.unwrap();
    assert_eq!("PARTIAL_SUCCESS", created["status"]);
    assert_eq!(2, created["succeededCount"]);
    assert_eq!(1, created["failures"][0]["index"]);
    assert_eq!("CONFLICT", created["failures"][0]["error"]);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listings WHERE listing_source_id=$1")
            .bind(source.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(2, count);
    let patched=call(&registry,NAMES[1],json!({"listingSourceId":source,"products":[{"sourceListingId":"same","availability":"AVAILABLE"},{"sourceListingId":"missing","availability":"AVAILABLE"},{"sourceListingId":"same","availability":"SOLD_OUT"}]}),context.clone()).await.unwrap();
    assert_eq!("PARTIAL_SUCCESS", patched["status"]);
    assert_eq!(2, patched["succeededCount"]);
    assert_eq!(1, patched["failures"][0]["index"]);
    let availability:String=sqlx::query_scalar("SELECT availability FROM product_listings WHERE listing_source_id=$1 AND source_listing_id='same'").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!("SOLD_OUT", availability);
    let withdrawn=call(&registry,NAMES[3],json!({"listingSourceId":source,"products":[{"sourceListingId":"same"},{"sourceListingId":"missing"},{"sourceListingId":"same"}]}),context.clone()).await.unwrap();
    assert_eq!("PARTIAL_SUCCESS", withdrawn["status"]);
    assert_eq!(true, withdrawn["successes"][0]["changed"]);
    assert_eq!(false, withdrawn["successes"][1]["changed"]);
    let restored=call(&registry,NAMES[2],json!({"listingSourceId":source,"products":[{"sourceListingId":"same","availability":"AVAILABLE"},{"sourceListingId":"same","availability":"RESERVED"},product("new")]}),context.clone()).await.unwrap();
    assert_eq!("COMPLETED", restored["status"]);
    assert_eq!("UPDATED", restored["successes"][0]["outcome"]);
    assert_eq!("CREATED", restored["successes"][2]["outcome"]);
    let availability:String=sqlx::query_scalar("SELECT availability FROM product_listings WHERE listing_source_id=$1 AND source_listing_id='same'").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!("RESERVED", availability);
    for name in NAMES {
        let mut unscoped = context.clone();
        unscoped.operation.principal = Principal::DelegatedUser {
            user_id,
            capabilities: BTreeSet::new(),
        };
        let report=call(&registry,name,json!({"listingSourceId":source,"products":[if name==NAMES[0] {product("forbidden")} else {json!({"sourceListingId":"same"})}]}),unscoped).await.unwrap();
        assert_eq!("FAILED", report["status"]);
        assert_eq!("FORBIDDEN", report["failures"][0]["error"]);
    }
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    for name in NAMES {
        let report=call(&registry,name,json!({"listingSourceId":source,"products":[if name==NAMES[0] {product("revoked")} else {json!({"sourceListingId":"same"})}]}),context.clone()).await.unwrap();
        assert_eq!("FAILED", report["status"]);
        assert_eq!("FORBIDDEN", report["failures"][0]["error"]);
    }
}
