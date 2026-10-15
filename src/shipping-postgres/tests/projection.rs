use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::Value;
use shipping_core::fixtures::gallery_terms;
use shipping_postgres::projection::SqlxShippingProjectionStoreFactory;
use shipping_service::{assessment::assess_listing, projection::*};
use sqlx::PgPool;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
mod support;
use support::*;
fn handler(pool: PgPool) -> impl ProjectListingShippingUseCase {
    ProjectListingShippingHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxShippingProjectionStoreFactory,
    )
}
async fn drain(pool: &PgPool, limit: u16) {
    for _ in 0..100 {
        handler(pool.clone()).execute(None, limit).await.unwrap();
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE pending)")
                .fetch_one(pool)
                .await
                .unwrap();
        if !pending {
            return;
        }
    }
    panic!("shipping fanout did not complete");
}
async fn summary(pool: &PgPool, id: ProductListingId) -> Value {
    sqlx::query_scalar("SELECT summary FROM product_listing_shipping WHERE product_listing_id=$1")
        .bind(id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn profile_update(pool: &PgPool, id: shipping_core::profile::ShippingProfileId, amount: u64) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET management_revision=management_revision+1,terms_revision=terms_revision+1,terms=$2 WHERE shipping_profile_id=$1").bind(id.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(amount)).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,terms_revision,change_kind,actor) SELECT listing_source_id,'PROFILE',shipping_profile_id,management_revision,terms_revision,'REPLACED','test' FROM shipping_profiles WHERE shipping_profile_id=$1").bind(id.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}
async fn claim(pool: &PgPool, kind: &str, id: String) -> ShippingClaim {
    let work: uuid::Uuid = sqlx::query_scalar(
        "SELECT work_id FROM listing_shipping_work WHERE dependency_kind=$1 AND dependency_id=$2",
    )
    .bind(kind)
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let c = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .claim(Some(work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    c
}
#[aura_integration_test(services=[SCHEMA])]
async fn literal_gallery_refresh_changes_only_painting_projection_and_keeps_canonical_state() {
    let pool = get_postgres_client().await;
    let (_, profile, ids) = seed(&pool).await;
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../shipping-json/fixtures/gallery-projection.json"
    ))
    .unwrap();
    let input: Value = serde_json::from_str(include_str!(
        "../../shipping-json/fixtures/gallery-postgres.json"
    ))
    .unwrap();
    let terms: Value =
        sqlx::query_scalar("SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(profile.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(input["before"], terms);
    let canonical:Vec<(uuid::Uuid,i64,time::OffsetDateTime)>=sqlx::query_as("SELECT product_listing_id,version,updated FROM product_listings ORDER BY product_listing_id").fetch_all(&pool).await.unwrap();
    drain(&pool, 100).await;
    assert_eq!(fixtures["paintingBefore"], summary(&pool, ids[0]).await);
    assert_eq!(fixtures["cabinet"], summary(&pool, ids[1]).await);
    let cabinet_fence:(i64,uuid::Uuid)=sqlx::query_as("SELECT projection_version,current_event_id FROM product_listings WHERE product_listing_id=$1").bind(ids[1].into_uuid()).fetch_one(&pool).await.unwrap();
    profile_update(&pool, profile, 9500).await;
    let terms: Value =
        sqlx::query_scalar("SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(profile.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(input["after"], terms);
    // Current origin reads already see new terms while indexed/derived predicates lag.
    let deps = read(&pool, &ids).await;
    let current = assess_listing(
        &deps,
        deps.listings.iter().find(|l| l.id == ids[0]).unwrap(),
        Default::default(),
    );
    assert_eq!(gallery_terms(9500), *current.effective.terms());
    assert_eq!(fixtures["paintingBefore"], summary(&pool, ids[0]).await);
    drain(&pool, 1).await;
    assert_eq!(fixtures["paintingAfter"], summary(&pool, ids[0]).await);
    assert_eq!(fixtures["cabinet"], summary(&pool, ids[1]).await);
    let after:Vec<(uuid::Uuid,i64,time::OffsetDateTime)>=sqlx::query_as("SELECT product_listing_id,version,updated FROM product_listings ORDER BY product_listing_id").fetch_all(&pool).await.unwrap();
    assert_eq!(canonical, after);
    let after_fence:(i64,uuid::Uuid)=sqlx::query_as("SELECT projection_version,current_event_id FROM product_listings WHERE product_listing_id=$1").bind(ids[1].into_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(cabinet_fence, after_fence);
    let basis: Value = sqlx::query_scalar(
        "SELECT dependency FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[1].into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(2, basis["profile"]["managementRevision"]);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE event_type='PRODUCT_LISTING_CHANGED'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(0, count);
}
#[aura_integration_test(services=[SCHEMA])]
async fn invalidation_during_claim_and_lease_expiry_fence_checkpoints_atomically() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    sqlx::query("SELECT enqueue_listing_shipping('PROFILE',$1)")
        .bind(profile.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    let claim = claim(&pool, "PROFILE", profile.into_uuid().to_string()).await;
    // Independent writer changes current authority while the old worker has no transaction.
    profile_update(&pool, profile, 9500).await;
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let page = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .page(&claim, 1)
        .await
        .unwrap();
    assert!(matches!(
        SqlxShippingProjectionStoreFactory
            .in_transaction(&mut tx)
            .checkpoint(&claim, &page)
            .await,
        Err(ShippingProjectionError::Superseded)
    ));
    drop(tx);
    let fresh = claim_work(&pool, source).await;
    sqlx::query("UPDATE listing_shipping_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(fresh.work_id.into_uuid()).execute(&pool).await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let page = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .page(&fresh, 1)
        .await
        .unwrap();
    assert!(matches!(
        SqlxShippingProjectionStoreFactory
            .in_transaction(&mut tx)
            .checkpoint(&fresh, &page)
            .await,
        Err(ShippingProjectionError::Superseded)
    ));
    drop(tx);
    drain(&pool, 1).await;
    assert_eq!(
        9500,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    assert_eq!(
        0,
        handler(pool.clone())
            .execute(Some(fresh.work_id), 1)
            .await
            .unwrap()
    );
}
async fn claim_work(pool: &PgPool, source: listing_source_core::ListingSourceId) -> ShippingClaim {
    sqlx::query("SELECT enqueue_listing_shipping('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(pool)
        .await
        .unwrap();
    claim(pool, "SOURCE", source.into_uuid().to_string()).await
}
#[aura_integration_test(services=[SCHEMA])]
async fn observation_expiry_and_connection_revocation_invalidate_without_listing_revisions() {
    let pool = get_postgres_client().await;
    let (_, profile, ids) = seed(&pool).await;
    drain(&pool, 100).await;
    // Observation-only validity is independent of normalized terms and canonical revisions.
    sqlx::query("UPDATE shipping_profiles SET expires_at=verified_at WHERE shipping_profile_id=$1")
        .bind(profile.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool, 100).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        summary(&pool, ids[1]).await["shipping_prohibited"]
            .as_bool()
            .unwrap()
    );
    // Revoke the item assertion too: independent item overrides no longer support certainty.
    sqlx::query("UPDATE product_listings SET delivery_expires_at=delivery_verified_at WHERE product_listing_id=$1").bind(ids[1].into_uuid()).execute(&pool).await.unwrap();
    drain(&pool, 100).await;
    assert!(
        !summary(&pool, ids[1]).await["shipping_prohibited"]
            .as_bool()
            .unwrap()
    );
    let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM product_listings")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(versions.iter().all(|v| *v == 1));
}
#[aura_integration_test(services=[SCHEMA])]
async fn invalid_storage_retains_work_and_rolls_back_derived_writes() {
    let pool = get_postgres_client().await;
    let (_, profile, _) = seed(&pool).await;
    sqlx::query("UPDATE shipping_profiles SET terms=jsonb_set(terms,'{schema}','\"FUTURE\"') WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&pool).await.unwrap();
    assert!(handler(pool.clone()).execute(None, 100).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_shipping")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
    let pending: bool = sqlx::query_scalar("SELECT bool_or(pending) FROM listing_shipping_work")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(pending);
}

async fn add_listing(
    pool: &PgPool,
    source: listing_source_core::ListingSourceId,
    id: ProductListingId,
) {
    let event = uuid::Uuid::now_v7();
    let raw = format!("item-{}-abcdef", id.as_uuid().simple());
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url)VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item')").bind(id.into_uuid()).bind(&raw).bind(event).bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time)VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,$3,now())").bind(event).bind(id.into_uuid()).bind(serde_json::json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":raw,"title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/item","imageCount":0,"auction":null})).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}
#[aura_integration_test(services=[SCHEMA])]
async fn fanout_restarts_on_profile_change_and_new_listing_behind_cursor_has_own_work() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    drain(&pool, 100).await;
    let claim = claim_work(&pool, source).await;
    // Complete one page, preserving a nonempty cursor for this source traversal.
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    let page = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .page(&claim, 1)
        .await
        .unwrap();
    assert!(!page.complete);
    SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .checkpoint(&claim, &page)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let new: ProductListingId = uuid::Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f75")
        .unwrap()
        .try_into()
        .unwrap();
    assert!(new.into_uuid() < page.after_listing.unwrap());
    add_listing(&pool, source, new).await;
    profile_update(&pool, profile, 9500).await;
    // The remaining old source page and independent profile/listing pages all reread current records.
    drain(&pool, 1).await;
    for id in [ids[0], new] {
        assert_eq!(
            9500,
            summary(&pool, id).await["country_offers"][0]["per_item_price"]["amount"]
        );
    }
    let independent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE dependency_kind='LISTING' AND dependency_id=$1 AND NOT pending)").bind(new.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    assert!(independent);
}
#[aura_integration_test(services=[SCHEMA])]
async fn retirement_default_switch_and_deleted_source_cannot_be_restored_by_old_work() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    drain(&pool, 100).await;
    sqlx::query("SELECT enqueue_listing_shipping('PROFILE',$1)")
        .bind(profile.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    let old = claim(&pool, "PROFILE", profile.into_uuid().to_string()).await;
    let replacement = shipping_core::profile::ShippingProfileId::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO shipping_profiles(shipping_profile_id,listing_source_id,current_source_id,management_revision,terms_revision,lifecycle,name,terms)SELECT $2,listing_source_id,current_source_id,1,1,'ACTIVE','Replacement',$3 FROM shipping_profiles WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(replacement.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(9500)).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE shipping_source_defaults SET shipping_profile_id=$2,revision=revision+1 WHERE listing_source_id=$1").bind(source.into_uuid()).bind(replacement.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor)SELECT listing_source_id,'SOURCE_DEFAULT',listing_source_id,revision,'DEFAULT_ASSIGNED','test' FROM shipping_source_defaults WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET lifecycle='RETIRED',management_revision=2,terms_revision=2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    // Expire the old claim: its eventual wake-up can only acquire current work.
    sqlx::query("UPDATE listing_shipping_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(old.work_id.into_uuid()).execute(&pool).await.unwrap();
    drain(&pool, 1).await;
    assert_eq!(
        9500,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    assert_eq!(
        0,
        handler(pool.clone())
            .execute(Some(old.work_id), 100)
            .await
            .unwrap()
    );
    // Explicit retired reference does not fall back to the replacement default.
    let delivery = product_listing_core::delivery::ListingDelivery::new(
        source,
        Default::default(),
        Some(shipping_core::selection::ProfileSelection::Explicit(
            profile,
        )),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=$2 WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .bind(shipping_json::listing_delivery::encode::<References>(&delivery).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    drain(&pool, 100).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sqlx::query("DELETE FROM product_listings WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool, 100).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_shipping")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, count);
    let deleted: bool = sqlx::query_scalar(
        "SELECT source_deleted FROM shipping_source_defaults WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(deleted);
}
#[aura_integration_test(services=[SCHEMA])]
async fn provider_connection_invalidation_and_time_expiry_recover_with_lost_wakeups() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    let integration = product_listing_core::delivery::DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Shopify,
        domain: listing_source_core::Domain::try_from("gallery.myshopify.com").unwrap(),
    };
    sqlx::query("INSERT INTO listing_source_ingestion_methods(listing_source_id,ingestion_method)VALUES($1,'SHOPIFY')").bind(source.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations(listing_source_id,domain)VALUES($1,'gallery.myshopify.com')").bind(source.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET integration_method='SHOPIFY',integration_domain='gallery.myshopify.com' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&pool).await.unwrap();
    let delivery = product_listing_core::delivery::ListingDelivery::new(
        source,
        integration,
        Some(shipping_core::selection::ProfileSelection::InheritSourceDefault),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=$2 WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .bind(shipping_json::listing_delivery::encode::<References>(&delivery).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    drain(&pool, 100).await;
    assert_eq!(
        8000,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1 AND ingestion_method='SHOPIFY'").bind(source.into_uuid()).execute(&pool).await.unwrap();
    drain(&pool, 100).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Restore current authority, then let a retained evidence deadline expire without writes.
    sqlx::query("INSERT INTO listing_source_ingestion_methods(listing_source_id,ingestion_method)VALUES($1,'SHOPIFY')").bind(source.into_uuid()).execute(&pool).await.unwrap();
    let expiry: time::OffsetDateTime = sqlx::query_scalar("UPDATE shipping_profiles SET expires_at=clock_timestamp()+interval '5 seconds' WHERE shipping_profile_id=$1 RETURNING expires_at")
        .bind(profile.into_uuid()).fetch_one(&pool).await.unwrap();
    drain(&pool, 100).await;
    assert_eq!(
        8000,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    let remaining =
        std::time::Duration::try_from(expiry - time::OffsetDateTime::now_utc()).unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(20)).await;
    // Cleanup neither needs the original wake-up nor a whole-import completion marker.
    drain(&pool, 100).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn source_switch_replaces_reverse_dependencies_and_fences_previous_assignment_work() {
    let pool = get_postgres_client().await;
    let (_, old_profile, ids) = seed(&pool).await;
    let (source, profile, _) = seed(&pool).await;
    profile_update(&pool, profile, 9500).await;
    drain(&pool, 100).await;
    sqlx::query("SELECT enqueue_listing_shipping('LISTING',$1)")
        .bind(ids[0].into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    let old = claim(&pool, "LISTING", ids[0].into_uuid().to_string()).await;
    sqlx::query("UPDATE product_listings SET listing_source_id=$2,version=version+1 WHERE product_listing_id=$1")
        .bind(ids[0].into_uuid()).bind(source.into_uuid()).execute(&pool).await.unwrap();
    // A duplicate old wake-up acquires the new generation and reads the new source.
    handler(pool.clone())
        .execute(Some(old.work_id), 100)
        .await
        .unwrap();
    assert_eq!(
        9500,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    let refs: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT dependency_id FROM product_listing_shipping_dependencies WHERE product_listing_id=$1 AND dependency_kind='PROFILE'",
    ).bind(ids[0].into_uuid()).fetch_all(&pool).await.unwrap();
    assert_eq!(vec![profile.into_uuid()], refs);
    profile_update(&pool, old_profile, 12000).await;
    drain(&pool, 1).await;
    assert_eq!(
        9500,
        summary(&pool, ids[0]).await["country_offers"][0]["per_item_price"]["amount"]
    );
    assert_eq!(
        0,
        handler(pool.clone())
            .execute(Some(old.work_id), 100)
            .await
            .unwrap()
    );
}
