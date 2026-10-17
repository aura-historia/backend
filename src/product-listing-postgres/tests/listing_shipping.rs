mod support;
use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{delivery::ListingDelivery, product_listing_id::ProductListingId};
use product_listing_postgres::shipping_projection::SqlxShippingProjectionStoreFactory;
use product_listing_service::shipping_projection::*;
use serde_json::Value;
use shipping_core::{
    fixtures::gallery_terms,
    profile::ShippingProfileId,
    selection::{ComponentOverride, DeliveryOverrides, ProfileSelection},
    terms::ShippingPolicy,
};
use shipping_postgres::assessment::SqlxAssessmentReaderFactory;
use shipping_service::assessment::*;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use uuid::Uuid;
const SCHEMA: Postgres = Postgres::new("migrations");
struct References;
impl shipping_json::ShippingTermsReferenceCodec for References {
    fn decode_party(v: &str) -> Result<PartyId, application::error::BoxError> {
        Ok(PartyId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_party(v: PartyId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_location(
        v: &str,
    ) -> Result<party_core::party_location_id::PartyLocationId, application::error::BoxError> {
        Ok(party_core::party_location_id::PartyLocationId::try_from(
            v.parse::<uuid::Uuid>()?,
        )?)
    }
    fn encode_location(v: party_core::party_location_id::PartyLocationId) -> String {
        v.into_uuid().to_string()
    }
}
impl shipping_json::listing_delivery::DeliveryReferenceCodec for References {
    fn decode_source(v: &str) -> Result<ListingSourceId, application::error::BoxError> {
        Ok(ListingSourceId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_source(v: ListingSourceId) -> String {
        v.into_uuid().to_string()
    }
    fn decode_profile(v: &str) -> Result<ShippingProfileId, application::error::BoxError> {
        Ok(ShippingProfileId::try_from(v.parse::<uuid::Uuid>()?)?)
    }
    fn encode_profile(v: ShippingProfileId) -> String {
        v.into_uuid().to_string()
    }
}
async fn seed(p: &PgPool) -> (ListingSourceId, ShippingProfileId, Vec<ProductListingId>) {
    let source = ListingSourceId::new();
    let owner = PartyId::new();
    let profile = ShippingProfileId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name)VALUES($1,$2,'Gallery')")
        .bind(owner.into_uuid())
        .bind(format!("gallery-{}", owner.as_uuid().simple()))
        .execute(p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id)VALUES($1,$2,'Gallery',$3)").bind(source.into_uuid()).bind(format!("source-{}",source.as_uuid().simple())).bind(owner.into_uuid()).execute(p).await.unwrap();
    sqlx::query("INSERT INTO shipping_profiles(shipping_profile_id,listing_source_id,current_source_id,management_revision,terms_revision,lifecycle,name,terms)VALUES($1,$2,$2,1,1,'ACTIVE','Gallery',$3)").bind(profile.into_uuid()).bind(source.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(8000)).unwrap()).execute(p).await.unwrap();
    sqlx::query("INSERT INTO shipping_source_defaults(listing_source_id,current_source_id,shipping_profile_id,revision)VALUES($1,$1,$2,2)").bind(source.into_uuid()).bind(profile.into_uuid()).execute(p).await.unwrap();
    let mut ids = Vec::new();
    for cabinet in [false, true] {
        let id = ProductListingId::new();
        let event = uuid::Uuid::new_v4();
        let delivery = if cabinet {
            Some(
                shipping_json::listing_delivery::encode::<References>(
                    &ListingDelivery::new(
                        source,
                        Default::default(),
                        Some(ProfileSelection::InheritSourceDefault),
                        Default::default(),
                        DeliveryOverrides {
                            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
                            collection: Default::default(),
                        },
                    )
                    .unwrap(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let mut listing_tx = p.begin().await.unwrap();
        sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url,delivery_configuration)VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item',$5)").bind(id.into_uuid()).bind(format!("item-{}-abcdef",id.as_uuid().simple())).bind(event).bind(source.into_uuid()).bind(delivery).execute(&mut *listing_tx).await.unwrap();
        sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time)VALUES($1,$2,'PRODUCT_LISTING_DISCOVERED','DOMAIN',1,$3,now())").bind(event).bind(id.into_uuid()).bind(serde_json::json!({"listingSourceId":source.into_uuid().to_string(),"sourceListingId":format!("item-{}-abcdef",id.as_uuid().simple()),"title":null,"description":null,"pricing":{"price":null,"priceEstimateMin":null,"priceEstimateMax":null},"availability":null,"url":"https://example.test/item","imageCount":0,"auction":null})).execute(&mut *listing_tx).await.unwrap();
        listing_tx.commit().await.unwrap();
        ids.push(id);
    }
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor) VALUES($1,'SOURCE_DEFAULT',$1,2,'DEFAULT_ASSIGNED','SERVICE:test')").bind(source.into_uuid()).execute(p).await.unwrap();
    (source, profile, ids)
}
fn projector(pool: &PgPool) -> impl ProjectListingShippingUseCase {
    ProjectListingShippingHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxShippingProjectionStoreFactory,
    )
}
async fn drain(pool: &PgPool) {
    for _ in 0..100 {
        projector(pool).execute(None, 100).await.unwrap();
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE pending)")
                .fetch_one(pool)
                .await
                .unwrap();
        if !pending {
            return;
        }
    }
    panic!("shipping fan-out did not converge");
}
async fn summary(pool: &PgPool, id: ProductListingId) -> Value {
    sqlx::query_scalar("SELECT summary FROM product_listing_shipping WHERE product_listing_id=$1")
        .bind(id.into_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}
fn fixture(name: &str) -> Value {
    let raw = match name {
        "painting8000" => include_str!("fixtures/shipping/painting-projection-8000.json"),
        "painting9500" => include_str!("fixtures/shipping/painting-projection-9500.json"),
        "cabinet" => include_str!("fixtures/shipping/cabinet-projection.json"),
        "postgres8000" => include_str!("fixtures/shipping/gallery-postgres-8000.json"),
        "postgres9500" => include_str!("fixtures/shipping/gallery-postgres-9500.json"),
        _ => panic!("unknown fixture"),
    };
    serde_json::from_str(raw).unwrap()
}
async fn replace_profile(pool: &PgPool, profile: ShippingProfileId, amount: u64) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=management_revision+1,terms_revision=terms_revision+1 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(amount)).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,terms_revision,change_kind,actor) SELECT listing_source_id,'PROFILE',shipping_profile_id,management_revision,terms_revision,'REPLACED','SERVICE:test' FROM shipping_profiles WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
}
#[aura_integration_test(services=[SCHEMA])]
async fn gallery_public_change_advances_only_painting_projection_and_cabinet_dependency_refresh_is_silent()
 {
    let pool = get_postgres_client().await;
    let (_, profile, ids) = seed(&pool).await;
    let terms: Value =
        sqlx::query_scalar("SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(profile.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(fixture("postgres8000"), terms);
    drain(&pool).await;
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    assert_eq!(fixture("cabinet"), summary(&pool, ids[1]).await);
    let before:Vec<(Uuid,i64,OffsetDateTime,i64,Uuid)>=sqlx::query_as("SELECT product_listing_id,version,updated,projection_version,current_event_id FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id").bind(ids.iter().map(|i|i.into_uuid()).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap();
    let basis: Value = sqlx::query_scalar(
        "SELECT dependency FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[1].into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    replace_profile(&pool, profile, 9500).await;
    // The existing source reader already assesses current PostgreSQL before projection work.
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let snapshot = SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&ids, None)
        .await
        .unwrap();
    let current = assess_snapshot(
        &snapshot,
        ids.iter()
            .map(|id| AssessListingRequest {
                listing_id: *id,
                input: Default::default(),
            })
            .collect(),
        None,
    );
    assert_eq!(
        9500,
        u64::from(
            derive(&current[0].effective, &[]).country_offers[0]
                .per_item_fixed
                .unwrap()
                .monetary_amount
        )
    );
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(fixture("painting9500"), summary(&pool, ids[0]).await);
    assert_eq!(fixture("cabinet"), summary(&pool, ids[1]).await);
    let after:Vec<(Uuid,i64,OffsetDateTime,i64,Uuid)>=sqlx::query_as("SELECT product_listing_id,version,updated,projection_version,current_event_id FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id").bind(ids.iter().map(|i|i.into_uuid()).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap();
    for (old, new) in before.iter().zip(&after) {
        assert_eq!((old.0, old.1, old.2), (new.0, new.1, new.2));
        if new.0 == ids[0].into_uuid() {
            assert_eq!(old.3 + 1, new.3);
            assert_ne!(old.4, new.4);
        } else {
            assert_eq!(old, new);
        }
    }
    let next_basis: Value = sqlx::query_scalar(
        "SELECT dependency FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[1].into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(basis, next_basis);
    assert_eq!(
        fixture("postgres9500"),
        sqlx::query_scalar::<_, Value>(
            "SELECT terms FROM shipping_profiles WHERE shipping_profile_id=$1"
        )
        .bind(profile.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    // Replay/rebuild with the same public summary creates no additional events.
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE event_type='ENRICHMENT_SHIPPING_UPDATED'").fetch_one(&pool).await.unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('PROFILE',$1)")
        .bind(profile.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool).await;
    assert_eq!(count,sqlx::query_scalar::<_,i64>("SELECT count(*) FROM product_listing_events WHERE event_type='ENRICHMENT_SHIPPING_UPDATED'").fetch_one(&pool).await.unwrap());
}
#[aura_integration_test(services=[SCHEMA])]
async fn duplicate_claims_expired_leases_and_invalidation_fence_transactional_checkpoints() {
    let pool = get_postgres_client().await;
    let (source, _, ids) = seed(&pool).await;
    let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let claim = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .claim(Some(work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        projector(&pool)
            .execute(Some(work.try_into().unwrap()), 1)
            .await,
        Err(ShippingProjectionError::Deferred)
    ));
    sqlx::query("UPDATE listing_shipping_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(work).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxShippingProjectionStoreFactory
            .in_transaction(&mut tx)
            .page(&claim, 1)
            .await,
        Err(ShippingProjectionError::Superseded)
    ));
    drop(tx);
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let claim = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .claim(Some(work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let page = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .page(&claim, 100)
        .await
        .unwrap();
    SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .apply(&[ShippingWrite {
            listing_id: ids[0],
            summary: EffectiveListingShipping {
                prohibited: true,
                ..Default::default()
            },
            valid_until: None,
        }])
        .await
        .unwrap();
    // Independent session invalidates a page after its work was read. Its checkpoint
    // fails and the complete summary/event/fence transaction must roll back.
    sqlx::query("SELECT enqueue_listing_shipping('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        SqlxShippingProjectionStoreFactory
            .in_transaction(&mut tx)
            .checkpoint(&claim, &page)
            .await
            .is_err()
    );
    drop(tx);
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM product_listing_shipping WHERE product_listing_id=$1)"
        )
        .bind(ids[0].into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    drain(&pool).await;
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    assert!(
        projector(&pool)
            .execute(Some(work.try_into().unwrap()), 100)
            .await
            .is_ok()
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn profile_updates_during_fanout_and_listings_behind_cursor_have_independent_work() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    projector(&pool)
        .execute(Some(work.try_into().unwrap()), 1)
        .await
        .unwrap();
    let cursor: Uuid =
        sqlx::query_scalar("SELECT after_listing FROM listing_shipping_work WHERE work_id=$1")
            .bind(work)
            .fetch_one(&pool)
            .await
            .unwrap();
    replace_profile(&pool, profile, 9500).await;
    let new_id = ProductListingId::try_from(
        Uuid::parse_str("01900000-0000-7000-8000-000000000001").unwrap(),
    )
    .unwrap();
    assert!(new_id.into_uuid() < cursor);
    let event = domain_primitives::event_id::EventId::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url) VALUES($1,'late-painting-abcdef',$2,$2,$2,$3,'late-painting','ACTIVE','https://example.test/late')").bind(new_id.into_uuid()).bind(event.into_uuid()).bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) SELECT $1,$2,event_type,event_group,event_type_schema_version,payload,now() FROM product_listing_events WHERE product_listing_id=$3 AND event_group='DOMAIN' LIMIT 1").bind(event.into_uuid()).bind(new_id.into_uuid()).bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    for id in [ids[0], new_id] {
        assert_eq!(fixture("painting9500"), summary(&pool, id).await);
    }
    assert_eq!(fixture("cabinet"), summary(&pool, ids[1]).await);
}
#[aura_integration_test(services=[SCHEMA])]
async fn observation_authority_and_deadline_changes_refresh_without_canonical_listing_changes() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    drain(&pool).await;
    let versions:Vec<(Uuid,i64)>=sqlx::query_as("SELECT product_listing_id,version FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id").bind(ids.iter().map(|id|id.into_uuid()).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET verified_at=now()-interval '2 days',fresh_until=now()-interval '1 day',expires_at=clock_timestamp()+interval '1 day' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_profile_import_state(shipping_profile_id,listing_source_id,producer_hash,object_hash,owner_hash,source_revision,sequence,observation_revision) SELECT $1,$2,decode(repeat('ab',32),'hex'),decode(repeat('cd',32),'hex'),decode(repeat('ab',32),'hex'),revision,1,1 FROM shipping_import_authority WHERE listing_source_id=$2").bind(profile.into_uuid()).bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'PROFILE',$2,1,'OBSERVED')").bind(source.into_uuid()).bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    let deadline: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT valid_until FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(deadline.is_some());
    // Make the stored deadline due without writing an invalidation signal. Hourly
    // maintenance independently discovers it and removes expired inherited evidence.
    sqlx::query("UPDATE shipping_profiles SET expires_at=now()-interval '1 hour' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_listing_shipping SET valid_until=now()-interval '1 second' WHERE product_listing_id=ANY($1)").bind(ids.iter().map(|id|id.into_uuid()).collect::<Vec<_>>()).execute(&pool).await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(true, summary(&pool, ids[1]).await["prohibited"]);
    // An observation-only refresh renews eligibility; the immutable subject revision
    // signal is sufficient even though profile terms and listing revisions stay fixed.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET verified_at=now()-interval '1 minute',fresh_until=now()+interval '1 hour',expires_at=now()+interval '2 hours' WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE shipping_profile_import_state SET observation_revision=2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'PROFILE',$2,2,'OBSERVED')").bind(source.into_uuid()).bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    sqlx::query("SELECT fence_shipping_imports($1)")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(true, summary(&pool, ids[1]).await["prohibited"]);
    // Manual ownership is independent of a revoked producer authority.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profile_import_state SET owner_hash=NULL,observation_revision=3 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'PROFILE',$2,3,'OWNERSHIP_CHANGED')").bind(source.into_uuid()).bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(fixture("painting8000"), summary(&pool, ids[0]).await);
    assert_eq!(versions,sqlx::query_as::<_,(Uuid,i64)>("SELECT product_listing_id,version FROM product_listings WHERE product_listing_id=ANY($1) ORDER BY product_listing_id").bind(ids.iter().map(|id|id.into_uuid()).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap());
}
#[aura_integration_test(services=[SCHEMA])]
async fn retirement_default_switch_withdrawal_and_source_removal_cannot_be_restored_by_old_work() {
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    drain(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET lifecycle='RETIRED',management_revision=management_revision+1,terms_revision=terms_revision+1 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,terms_revision,change_kind,actor) SELECT listing_source_id,'PROFILE',shipping_profile_id,management_revision,terms_revision,'RETIRED','SERVICE:test' FROM shipping_profiles WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        summary(&pool, ids[1]).await["prohibited"]
            .as_bool()
            .unwrap()
    );
    let replacement = ShippingProfileId::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO shipping_profiles(shipping_profile_id,listing_source_id,current_source_id,management_revision,terms_revision,lifecycle,name,terms) VALUES($1,$2,$2,1,1,'ACTIVE','Replacement',$3)").bind(replacement.into_uuid()).bind(source.into_uuid()).bind(fixture("postgres9500")).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE shipping_source_defaults SET shipping_profile_id=$2,revision=revision+1 WHERE listing_source_id=$1").bind(source.into_uuid()).bind(replacement.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,change_kind,actor) SELECT listing_source_id,'SOURCE_DEFAULT',listing_source_id,revision,'DEFAULT_ASSIGNED','SERVICE:test' FROM shipping_source_defaults WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('PROFILE',$1)")
        .bind(profile.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool).await;
    assert_eq!(fixture("painting9500"), summary(&pool, ids[0]).await);
    // A retained explicit reference that no longer exists must never fall back to
    // the source's valid replacement default, even under old profile wake-ups.
    let missing = ShippingProfileId::new();
    let delivery = ListingDelivery::new(
        source,
        Default::default(),
        Some(ProfileSelection::Explicit(missing)),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=$2 WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .bind(shipping_json::listing_delivery::encode::<References>(&delivery).unwrap())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('LISTING',$1)")
        .bind(ids[0].into_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE product_listings SET lifecycle='WITHDRAWN' WHERE product_listing_id=$1")
        .bind(ids[0].into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('LISTING',$1)")
        .bind(ids[0].into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool).await;
    assert_eq!(false, summary(&pool, ids[0]).await["prohibited"]);
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
    drain(&pool).await;
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM product_listing_shipping WHERE product_listing_id=ANY($1))"
        )
        .bind(ids.iter().map(|id| id.into_uuid()).collect::<Vec<_>>())
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}

async fn accepted_country_resolution(
    pool: &sqlx::PgPool,
    owner: Uuid,
    location: Uuid,
) -> Result<geo_service::resolution::ResolutionId, Box<dyn std::error::Error + Send + Sync>> {
    use geo_postgres::SqlxResolutionRepositoryFactory;
    use geo_service::{geocoding::*, resolution::*};
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted) VALUES('TEST_G6',true,true,true)").execute(pool).await?;
    let input = ResolutionInput::new(
        GeocodingRequest::new(geo::AddressText::new("Shared eligible address")?),
        ResolutionScope::new(ScopeKind::Party, Some(owner))?,
        RetentionPurpose::PartyLocation,
    )?;
    let binding = ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::PartyLocation, location)?,
        input_revision: 1,
        guard: 1,
    };
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let resolution_id = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .bind(&input, binding)
        .await?;
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(binding.subject)
        .await?
        .ok_or("missing binding")?;
    tx.commit().await?;
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(view.work, "TEST_G6")
        .await?
    else {
        return Err("missing resolution claim".into());
    };
    tx.commit().await?;
    let candidate = GeocodingCandidate::new(
        geo::DerivedGeography::new(
            geo::GeographicDescription::new(None, Some(geo::CountryCode::DEU), None, None)?,
            None,
        ),
        vec![],
        None,
        GeocodingProvenance {
            provider: "TEST",
            product: "TEST",
            data_source: "TEST",
            region: "TEST".into(),
            interpretation_profile: "TEST_G6",
            external_reference: "test-record".into(),
            attribution_url: "https://example.test/attribution",
        },
    );
    // Use the service policy to retain the source candidate alongside its
    // accepted administrative projection.
    let evidence = evaluate(
        &input,
        GeocodingResult {
            outcome: GeocodingOutcome::Candidates(CandidateSet::new(
                vec![candidate],
                input.request().candidate_limit(),
            )?),
            retention: ResultRetention::Storage,
        },
        "TEST_G6",
    )?;
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    assert!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&claim, ResolutionCompletion::Evidence(evidence))
            .await?
    );
    tx.commit().await?;
    Ok(resolution_id)
}

#[aura_integration_test(services=[SCHEMA])]
async fn explicit_sites_resolution_rights_and_expiry_invalidate_without_private_projection_data() {
    use shipping_core::{coverage::*, inputs::Applicability, terms::*};
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let location = Uuid::now_v7();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Private dispatch',NULL,'ACTIVE','SERVICE:test','SERVICE:test')").bind(location).bind(owner).execute(&pool).await.unwrap();
    let base = gallery_terms(8000);
    let ShippingPolicy::Coverage(c) = base.shipping().unwrap() else {
        unreachable!()
    };
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                c.release(),
                CoverageCompleteness::Partial,
                Applicability::default(),
                vec![],
                vec![
                    ShippingZone::new(
                        vec![DeliveryArea::Domestic],
                        Applicability::default(),
                        c.zones()[0].services().to_vec(),
                    )
                    .unwrap(),
                ],
                Some(DeliverySite::PartyLocation(location.try_into().unwrap())),
            )
            .unwrap(),
        )),
        Some(CollectionPolicy::new(
            Applicability::default(),
            CollectionDependency::Independent,
            Some(ShippingEligibility::Offered),
            Some(DeliverySite::PartyLocation(location.try_into().unwrap())),
            None,
        )),
    );
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=management_revision+1,terms_revision=terms_revision+1 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&terms).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_profile_location_references(shipping_profile_id,party_location_id,applicability) VALUES($1,$2,'DISPATCH'),($1,$2,'COLLECTION')").bind(profile.into_uuid()).bind(location).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_changes(listing_source_id,subject_kind,subject_id,revision,terms_revision,change_kind,actor) SELECT listing_source_id,'PROFILE',shipping_profile_id,management_revision,terms_revision,'REPLACED','SERVICE:test' FROM shipping_profiles WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let resolution = accepted_country_resolution(&pool, owner, location)
        .await
        .unwrap();
    drain(&pool).await;
    assert_eq!(
        "DE",
        summary(&pool, ids[0]).await["country_offers"][0]["country"]
    );
    let state:String=sqlx::query_scalar("SELECT (summary||dependency)::text FROM product_listing_shipping WHERE product_listing_id=$1").bind(ids[0].into_uuid()).fetch_one(&pool).await.unwrap();
    assert!(!state.contains("Shared eligible address"));
    assert!(!state.contains("Private dispatch"));
    // Geography holds the listing first, then reads resolution state. Shipping
    // must wait for that listing without holding a conflicting resolution lock.
    sqlx::query("INSERT INTO listing_source_location_defaults(listing_source_id,party_id,party_location_id,business_evidence,revision,updated_actor) VALUES($1,$2,$3,$4,2,'SERVICE:test')").bind(source.into_uuid()).bind(owner).bind(location).bind(serde_json::json!({"scope":"ASSOCIATED_BUSINESS","reference":"synthetic fixture"})).execute(&pool).await.unwrap();
    sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    let geo_work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_geography_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    let shipping_work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    use product_listing_service::geography::{
        GeographyProjectionStore, GeographyProjectionStoreFactory,
    };
    let mut geo_tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let geo_claim = product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory
        .in_transaction(&mut geo_tx)
        .claim(Some(geo_work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    geo_tx.commit().await.unwrap();
    let mut shipping_tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let shipping_claim = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut shipping_tx)
        .claim(Some(shipping_work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    shipping_tx.commit().await.unwrap();
    let mut geo_tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(geo_tx.connection())
        .await
        .unwrap();
    product_listing_postgres::geography::SqlxGeographyProjectionStoreFactory
        .in_transaction(&mut geo_tx)
        .page(&geo_claim, 100)
        .await
        .unwrap();
    let mut shipping_tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    {
        let mut shipping_page = std::pin::pin!(async {
            SqlxShippingProjectionStoreFactory
                .in_transaction(&mut shipping_tx)
                .page(&shipping_claim, 100)
                .await
        });
        support::assert_blocked(&pool, blocker, 1, shipping_page.as_mut())
            .await
            .unwrap();
        use geo_service::resolution::{
            ResolutionReader, ResolutionRepositoryFactory, ResolutionSubject, RetentionPurpose,
        };
        {
            let mut resolution_reader =
                geo_postgres::SqlxResolutionRepositoryFactory.resolution_reader(&mut geo_tx);
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                resolution_reader.inspect_many(&[ResolutionSubject::new(
                    RetentionPurpose::PartyLocation,
                    location,
                )
                .unwrap()]),
            )
            .await
            .unwrap()
            .unwrap();
        }
        geo_tx.commit().await.unwrap();
        shipping_page.await.unwrap();
    }
    drop(shipping_tx);
    // Remove this geography-only fixture before the later operator handoff.
    sqlx::query("DELETE FROM listing_source_location_defaults WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    // The claim-only phases survive the deliberately rolled-back proof pages.
    sqlx::query("UPDATE listing_shipping_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(shipping_work).execute(&pool).await.unwrap();
    // The page holds management authority until its atomic checkpoint commits.
    sqlx::query("SELECT enqueue_listing_shipping('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(&pool)
        .await
        .unwrap();
    let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let claim = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .claim(Some(work.try_into().unwrap()))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let _page = SqlxShippingProjectionStoreFactory
        .in_transaction(&mut tx)
        .page(&claim, 100)
        .await
        .unwrap();
    let mut management = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout='10ms'")
        .execute(&mut *management)
        .await
        .unwrap();
    let blocked = sqlx::query(
        "SELECT listing_source_id FROM listing_sources WHERE listing_source_id=$1 FOR UPDATE",
    )
    .bind(source.into_uuid())
    .fetch_all(&mut *management)
    .await
    .unwrap_err();
    assert_eq!(
        Some("55P03"),
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref()
    );
    management.rollback().await.unwrap();
    drop(tx);
    sqlx::query("UPDATE listing_shipping_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(work).execute(&pool).await.unwrap();
    sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false WHERE profile='TEST_G6'").execute(&pool).await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=true WHERE profile='TEST_G6'").execute(&pool).await.unwrap();
    drain(&pool).await;
    assert_eq!(
        "DE",
        summary(&pool, ids[0]).await["country_offers"][0]["country"]
    );
    let replacement_owner = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Replacement operator')",
    )
    .bind(replacement_owner)
    .bind(format!("replacement-{}", replacement_owner.simple()))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE listing_sources SET operator_party_id=$2,version=version+1 WHERE listing_source_id=$1").bind(source.into_uuid()).bind(replacement_owner).execute(&pool).await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(summary(&pool, ids[0]).await["collection"].is_null());
    sqlx::query("UPDATE listing_sources SET operator_party_id=$2,version=version+1 WHERE listing_source_id=$1").bind(source.into_uuid()).bind(owner).execute(&pool).await.unwrap();
    drain(&pool).await;
    assert_eq!(
        "DE",
        summary(&pool, ids[0]).await["country_offers"][0]["country"]
    );
    sqlx::query("UPDATE geo_resolutions SET valid_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1").bind(resolution.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_listing_shipping SET valid_until=clock_timestamp()-interval '1 second' WHERE product_listing_id=ANY($1)").bind(ids.iter().map(|id|id.into_uuid()).collect::<Vec<_>>()).execute(&pool).await.unwrap();
    drain(&pool).await;
    assert!(
        summary(&pool, ids[0]).await["country_offers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(location).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO party_location_changes(party_location_id,party_id,revision,input_revision,lifecycle,actor) SELECT party_location_id,party_id,revision,input_revision,lifecycle,'SERVICE:test' FROM party_locations WHERE party_location_id=$1").bind(location).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert!(summary(&pool, ids[0]).await["collection"].is_null());
    assert!(
        summary(&pool, ids[1]).await["prohibited"]
            .as_bool()
            .unwrap()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn bounded_source_rebuild_recreates_summaries_and_reverse_dependencies_without_listing_writes()
 {
    let pool = get_postgres_client().await;
    let (source, _, ids) = seed(&pool).await;
    drain(&pool).await;
    let before:Vec<(Uuid,i64,OffsetDateTime)>=sqlx::query_as("SELECT product_listing_id,version,updated FROM product_listings WHERE listing_source_id=$1 ORDER BY product_listing_id").bind(source.into_uuid()).fetch_all(&pool).await.unwrap();
    let expected = vec![summary(&pool, ids[0]).await, summary(&pool, ids[1]).await];
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM listing_shipping_dependencies WHERE product_listing_id=ANY($1)")
        .bind(ids.iter().map(|id| id.into_uuid()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("DELETE FROM product_listing_shipping WHERE product_listing_id=ANY($1)")
        .bind(ids.iter().map(|id| id.into_uuid()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT enqueue_listing_shipping('SOURCE',$1)")
        .bind(source.into_uuid().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    for _ in 0..3 {
        assert!(
            projector(&pool)
                .execute(Some(work.try_into().unwrap()), 1)
                .await
                .unwrap()
                <= 1
        );
    }
    assert_eq!(
        expected,
        vec![summary(&pool, ids[0]).await, summary(&pool, ids[1]).await]
    );
    assert_eq!(before,sqlx::query_as::<_,(Uuid,i64,OffsetDateTime)>("SELECT product_listing_id,version,updated FROM product_listings WHERE listing_source_id=$1 ORDER BY product_listing_id").bind(source.into_uuid()).fetch_all(&pool).await.unwrap());
    let references: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM listing_shipping_dependencies WHERE product_listing_id=ANY($1)",
    )
    .bind(ids.iter().map(|id| id.into_uuid()).collect::<Vec<_>>())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(4, references);
}

#[aura_integration_test(services=[SCHEMA])]
async fn component_observations_and_manual_handoff_recompute_applicability_without_listing_events()
{
    use shipping_core::{coverage::*, inputs::*, terms::DeliveryTerms};
    let pool = get_postgres_client().await;
    let (source, profile, ids) = seed(&pool).await;
    let class = SourceShippingClass::new(source, "painting".into()).unwrap();
    let base = gallery_terms(8000);
    let ShippingPolicy::Coverage(c) = base.shipping().unwrap() else {
        unreachable!()
    };
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                c.release(),
                c.completeness(),
                Applicability::new(Some(class.clone()), None, None, None).unwrap(),
                c.exclusions().to_vec(),
                c.zones().to_vec(),
                None,
            )
            .unwrap(),
        )),
        base.collection().cloned(),
    );
    let inputs = ListingRateInputs::new(
        Some(ClassMembership::new(vec![class], FactCompleteness::Complete).unwrap()),
        None,
    )
    .unwrap();
    let delivery = ListingDelivery::new(
        source,
        Default::default(),
        Some(ProfileSelection::InheritSourceDefault),
        inputs,
        Default::default(),
    )
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=management_revision+1,terms_revision=terms_revision+1 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&terms).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "UPDATE product_listings SET delivery_configuration=$2 WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .bind(shipping_json::listing_delivery::encode::<References>(&delivery).unwrap())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO listing_delivery_import_state(product_listing_id,component,owner_hash,source_revision,sequence,observation_revision,verified_at,fresh_until,expires_at) SELECT $1,'RATE_INPUTS',decode(repeat('ab',32),'hex'),revision,2,1,now()-interval '2 days',now()+interval '1 hour',now()+interval '2 hours' FROM shipping_import_authority WHERE listing_source_id=$2 ON CONFLICT(product_listing_id,component) DO UPDATE SET owner_hash=EXCLUDED.owner_hash,source_revision=EXCLUDED.source_revision,sequence=EXCLUDED.sequence,observation_revision=EXCLUDED.observation_revision,verified_at=EXCLUDED.verified_at,fresh_until=EXCLUDED.fresh_until,expires_at=EXCLUDED.expires_at").bind(ids[0].into_uuid()).bind(source.into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'LISTING',$2,1,'OBSERVED')").bind(source.into_uuid()).bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(
        "DE",
        summary(&pool, ids[0]).await["country_offers"][0]["country"]
    );
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1 AND event_group='DOMAIN'").bind(ids[0].into_uuid()).fetch_one(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE listing_delivery_import_state SET observation_revision=2,fresh_until=now()-interval '1 day',expires_at=now()-interval '1 hour' WHERE product_listing_id=$1 AND component='RATE_INPUTS'").bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'LISTING',$2,2,'OBSERVED')").bind(source.into_uuid()).bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    let expired = summary(&pool, ids[0]).await;
    assert!(expired["country_offers"].as_array().unwrap().is_empty());
    assert_eq!("OFFERED", expired["collection"]);
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE listing_delivery_import_state SET owner_hash=NULL,observation_revision=3,verified_at=NULL,fresh_until=NULL,expires_at=NULL WHERE product_listing_id=$1 AND component='RATE_INPUTS'").bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO shipping_validity_changes(listing_source_id,subject_kind,subject_id,revision,change_kind) VALUES($1,'LISTING',$2,3,'OWNERSHIP_CHANGED')").bind(source.into_uuid()).bind(ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    drain(&pool).await;
    assert_eq!(
        "DE",
        summary(&pool, ids[0]).await["country_offers"][0]["country"]
    );
    assert_eq!(count,sqlx::query_scalar::<_,i64>("SELECT count(*) FROM product_listing_events WHERE product_listing_id=$1 AND event_group='DOMAIN'").bind(ids[0].into_uuid()).fetch_one(&pool).await.unwrap());
}

#[aura_integration_test(services=[SCHEMA])]
async fn a_hundred_inline_listings_batch_two_hundred_independent_sites_in_one_snapshot() {
    use shipping_core::{coverage::ShippingCoverage, terms::*};
    let pool = get_postgres_client().await;
    let (source, _, seed_ids) = seed(&pool).await;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let base = gallery_terms(8000);
    let ShippingPolicy::Coverage(c) = base.shipping().unwrap() else {
        unreachable!()
    };
    let mut ids = vec![];
    let mut tx = pool.begin().await.unwrap();
    for _ in 0..100 {
        let dispatch = Uuid::now_v7();
        let collection = Uuid::now_v7();
        sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor) VALUES($1,$3,'Dispatch',$4,'ACTIVE','SERVICE:test','SERVICE:test'),($2,$3,'Collection',$4,'ACTIVE','SERVICE:test','SERVICE:test')").bind(dispatch).bind(collection).bind(owner).bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":null,"country":"DE","subdivision":null,"postal_code":null})).execute(&mut *tx).await.unwrap();
        let terms = DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    c.release(),
                    c.completeness(),
                    Default::default(),
                    vec![],
                    c.zones().to_vec(),
                    Some(DeliverySite::PartyLocation(dispatch.try_into().unwrap())),
                )
                .unwrap(),
            )),
            Some(CollectionPolicy::new(
                Default::default(),
                CollectionDependency::Independent,
                Some(ShippingEligibility::Offered),
                Some(DeliverySite::PartyLocation(collection.try_into().unwrap())),
                None,
            )),
        );
        let delivery = ListingDelivery::new(
            source,
            Default::default(),
            Some(ProfileSelection::inline(terms)),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        let id = ProductListingId::new();
        let event = domain_primitives::event_id::EventId::new();
        sqlx::query("INSERT INTO product_listings(product_listing_id,product_listing_title_slug_id,current_event_id,content_source_event_id,embedding_source_event_id,listing_source_id,source_listing_id,lifecycle,url,delivery_configuration) VALUES($1,$2,$3,$3,$3,$4,$2,'ACTIVE','https://example.test/item',$5)").bind(id.into_uuid()).bind(format!("item-{}-abcdef",id.as_uuid().simple())).bind(event.into_uuid()).bind(source.into_uuid()).bind(shipping_json::listing_delivery::encode::<References>(&delivery).unwrap()).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) SELECT $1,$2,event_type,event_group,event_type_schema_version,payload,now() FROM product_listing_events WHERE product_listing_id=$3 AND event_group='DOMAIN' LIMIT 1").bind(event.into_uuid()).bind(id.into_uuid()).bind(seed_ids[0].into_uuid()).execute(&mut *tx).await.unwrap();
        ids.push(id);
    }
    tx.commit().await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let deps = SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&ids, None)
        .await
        .unwrap();
    assert_eq!(100, deps.listings.len());
    assert_eq!(200, deps.locations.len());
    assert!(
        assess_snapshot(
            &deps,
            ids.iter()
                .map(|id| AssessListingRequest {
                    listing_id: *id,
                    input: Default::default()
                })
                .collect(),
            None
        )
        .iter()
        .all(|v| derive(
            &v.effective,
            &deps
                .locations
                .iter()
                .map(|l| l.location.clone())
                .collect::<Vec<_>>()
        )
        .collection
            == Some(ShippingEligibility::Offered))
    );
    tx.commit().await.unwrap();
    let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_shipping_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.into_uuid().to_string()).fetch_one(&pool).await.unwrap();
    assert!(
        projector(&pool)
            .execute(Some(work.try_into().unwrap()), 100)
            .await
            .unwrap()
            <= 100
    );
}
