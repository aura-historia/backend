use super::*;
use aura_historia_contracts::partner_product_listings::CreateProductListingData;
use aura_historia_runtime::{ProductListingIngestionDependencies, Runtime};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
async fn source(pool: &sqlx::PgPool) -> (uuid::Uuid, ListingSourceId) {
    let party = uuid::Uuid::now_v7();
    let source = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Delivery operator')",
    )
    .bind(party)
    .bind(format!("party-{party}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Delivery source',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid())).bind(party).execute(pool).await.unwrap();
    (party, source)
}
async fn grant(pool: &sqlx::PgPool, user: UserId, party: uuid::Uuid, source: ListingSourceId) {
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(pool).await.unwrap();
}
async fn create(
    runtime: &Runtime,
    source: ListingSourceId,
    id: &str,
    ctx: &InvocationContext,
) -> ProductListingId {
    let command = serde_json::from_value::<CreateProductListingData>(json!({"sourceListingId":id,"title":{"language":"en","text":"Painting"},"url":"https://example.test/painting","images":[]})).unwrap().into_command(source).unwrap();
    runtime
        .partner_product_listing_batches()
        .batches
        .create(&ctx.operation, vec![command])
        .await
        .unwrap()
        .items
        .pop()
        .unwrap()
        .result
        .unwrap()
        .product_listing_id
}
fn registry(runtime: &Runtime, publisher: Publisher) -> Registry {
    Registry::new(
        operations(
            runtime.product_listing_delivery().clone(),
            runtime
                .partner_product_listing_ingestion(ProductListingIngestionDependencies::new(
                    publisher,
                ))
                .submit
                .clone(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn prohibition(source: ListingSourceId, id: &str, version: u64) -> Value {
    json!({"listingSourceId":source,"sourceListingId":id,"expectedVersion":version,"delivery":{"state":"SET","value":{"integration":{"kind":"SOURCE"},"selection":{"state":"UNCHANGED"},"rateInputs":{"state":"UNCHANGED"},"shipping":{"state":"SET","value":{"kind":"REPLACE","value":{"kind":"PROHIBITED"}}},"collection":{"state":"UNCHANGED"}}}})
}
#[aura_integration_test(services=[SCHEMA], isolated=true)]
async fn should_use_source_local_identity_and_preserve_current_versions_changes_and_stale_noop_conflicts()
 {
    let pool = get_postgres_client().await;
    let (party, source) = source(&pool).await;
    let ctx = context();
    let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
        panic!("delegated")
    };
    grant(&pool, user_id, party, source).await;
    let runtime = Runtime::from_pool(pool.clone());
    let first = create(&runtime, source, "painting", &ctx).await;
    let local_uuid = first.into_uuid().to_string();
    let second = create(&runtime, source, &local_uuid, &ctx).await;
    let registry = registry(&runtime, Publisher::default());
    let query = |id: &str| json!({"listingSourceId":source,"sourceListingId":id});
    let initial = call(&registry, GET, query("painting"), ctx.clone())
        .await
        .unwrap();
    assert_eq!(
        initial,
        json!({"productListingId":first,"expectedVersion":1,"delivery":null})
    );
    let other = call(&registry, GET, query(&local_uuid), ctx.clone())
        .await
        .unwrap();
    assert_eq!(other["productListingId"], json!(second));
    assert_ne!(first, second);
    assert!(matches!(
        call(&registry, GET, query(&first.to_string()), ctx.clone()).await,
        Err(OperationError::NotFound)
    ));
    let changed = call(
        &registry,
        CHANGE,
        prohibition(source, "painting", 1),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        changed,
        json!({"productListingId":first,"expectedVersion":2,"outcome":"CHANGED"})
    );
    let unchanged = call(
        &registry,
        CHANGE,
        prohibition(source, "painting", 2),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        unchanged,
        json!({"productListingId":first,"expectedVersion":2,"outcome":"UNCHANGED"})
    );
    assert!(matches!(
        call(
            &registry,
            CHANGE,
            prohibition(source, "painting", 1),
            ctx.clone()
        )
        .await,
        Err(OperationError::Conflict(None))
    ));
    assert!(matches!(
        call(&registry, CHANGE, input(source, "painting"), ctx.clone()).await,
        Err(OperationError::Conflict(None))
    ));
    let current = call(&registry, GET, query("painting"), ctx.clone())
        .await
        .unwrap();
    assert_eq!(current["expectedVersion"], 2);
    assert_eq!(
        current["delivery"]["shipping"],
        json!({"kind":"REPLACE","value":{"kind":"PROHIBITED"}})
    );
    let events:i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1 AND event_group='DOMAIN'").bind(first.into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(events, 2);
}
#[aura_integration_test(services=[SCHEMA], isolated=true)]
async fn should_recheck_grants_for_reads_equal_changes_and_later_application_after_queue_admission()
{
    let pool = get_postgres_client().await;
    let (party, source) = source(&pool).await;
    let ctx = context();
    let Principal::DelegatedUser { user_id, .. } = ctx.operation.principal else {
        panic!("delegated")
    };
    grant(&pool, user_id, party, source).await;
    let runtime = Runtime::from_pool(pool.clone());
    let id = create(&runtime, source, "painting", &ctx).await;
    let publisher = Publisher::default();
    let registry = registry(&runtime, publisher.clone());
    let (_, foreign) = self::source(&pool).await;
    assert!(matches!(
        call(
            &registry,
            GET,
            json!({"listingSourceId":foreign,"sourceListingId":"painting"}),
            ctx.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    assert!(matches!(
        call(
            &registry,
            CHANGE,
            prohibition(foreign, "painting", 1),
            ctx.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    let ordinary = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(ordinary.into_uuid())
        .bind(format!("{ordinary}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    let mut consumer = ctx.clone();
    consumer.operation.principal = Principal::DelegatedUser {
        user_id: ordinary,
        capabilities: [
            CredentialCapability::ProductListingsRead,
            CredentialCapability::ProductListingsWrite,
        ]
        .into(),
    };
    assert!(matches!(
        call(
            &registry,
            GET,
            json!({"listingSourceId":source,"sourceListingId":"painting"}),
            consumer.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    assert!(matches!(
        call(&registry, CHANGE, input(source, "painting"), consumer).await,
        Err(OperationError::Forbidden)
    ));
    let report = call(
        &registry,
        SUBMIT,
        prohibition(source, "painting", 1),
        ctx.clone(),
    )
    .await
    .unwrap();
    assert_eq!(report["items"][0]["outcome"], "PUBLISHED");
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM product_listings WHERE product_listing_id=$1")
            .bind(id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(version, 1, "queue custody does not mutate a listing");
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        call(
            &registry,
            GET,
            json!({"listingSourceId":source,"sourceListingId":"painting"}),
            ctx.clone()
        )
        .await,
        Err(OperationError::Forbidden)
    ));
    assert!(matches!(
        call(&registry, CHANGE, input(source, "painting"), ctx.clone()).await,
        Err(OperationError::Forbidden)
    ));
    let ProductListingIngestionIntent::ChangeDelivery(command) =
        publisher.0.lock().unwrap()[0].intent.clone()
    else {
        panic!("delivery intent")
    };
    assert!(matches!(
        runtime
            .product_listing_delivery()
            .change
            .execute(&ctx.operation, command)
            .await,
        Err(ProductListingDeliveryError::Forbidden)
    ));
}
