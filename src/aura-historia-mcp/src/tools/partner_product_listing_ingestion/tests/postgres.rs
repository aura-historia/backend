use super::*;
use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
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
async fn should_check_shopify_source_grants_synchronously_and_keep_admission_separate_from_commit()
{
    let pool = get_postgres_client().await;
    let ctx = context();
    let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
        panic!("delegated actor")
    };
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let (party, source) = seed_source(&pool).await;
    let runtime = Runtime::from_pool(pool.clone());
    let publisher = Publisher::default();
    let registry = Registry::new(
        operations(
            runtime
                .partner_product_listing_ingestion(ProductListingIngestionDependencies::new(
                    publisher.clone(),
                ))
                .clone(),
        )
        .unwrap(),
    )
    .unwrap();
    let input = json!({"listingSourceId":source,"configuration":{"domain":"shop.example.test","currency":"EUR","language":"de"}});
    assert!(matches!(
        call(
            &registry,
            "putShopifyListingSourceIngestionConfiguration",
            input.clone(),
            ctx.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    grant_source(&pool, user_id, party, source).await;
    let created = call(
        &registry,
        "putShopifyListingSourceIngestionConfiguration",
        input.clone(),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert_eq!(created, json!({"created":true}));
    let stored:(String,Option<String>,Option<String>)=sqlx::query_as("SELECT domain,currency,language FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(
        stored,
        (
            "shop.example.test".into(),
            Some("EUR".into()),
            Some("de".into())
        )
    );
    let replaced = call(
        &registry,
        "putShopifyListingSourceIngestionConfiguration",
        json!({"listingSourceId":source,"configuration":{"domain":"shop.example.test"}}),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert_eq!(replaced, json!({"created":false}));
    let cleared:(Option<String>,Option<String>)=sqlx::query_as("SELECT currency,language FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(cleared, (None, None));
    let mut unscoped = ctx.clone();
    unscoped.operation.principal = Principal::DelegatedUser {
        user_id,
        capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
    };
    assert!(matches!(
        call(
            &registry,
            "putShopifyListingSourceIngestionConfiguration",
            input.clone(),
            unscoped
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        call(
            &registry,
            "putShopifyListingSourceIngestionConfiguration",
            input,
            ctx.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    // Admission intentionally does not look up grants or write canonical listings.
    let report=call(&registry,NAMES[0],json!({"listingSourceId":source,"idempotencyKey":"admit-with-revoked-grant","products":[product(NAMES[0],"queued")]}),ctx).await.unwrap();
    assert_eq!(report["acceptedCount"], 1);
    assert_eq!(1, publisher.sent.lock().unwrap().len());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listings WHERE listing_source_id=$1")
            .bind(source.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(0, count);
}
