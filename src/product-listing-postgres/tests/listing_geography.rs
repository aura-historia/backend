use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use geo_postgres::SqlxResolutionRepositoryFactory;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::{
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
    geography::{
        SqlxEffectiveListingGeographyBatchReader, SqlxGeographicQueryGate,
        SqlxGeographyProjectionStoreFactory,
    },
};
use product_listing_service::{
    geography::*,
    ports::{
        ProductListingSearchFilterMatchSourceReader,
        ProductListingSearchFilterMatchSourceReaderFactory,
    },
};
use serde_json::json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use uuid::Uuid;
const SCHEMA: Postgres = Postgres::new("migrations");
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn handler(pool: sqlx::PgPool) -> impl ProjectListingGeographyUseCase {
    ProjectListingGeographyHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxGeographyProjectionStoreFactory,
        SqlxResolutionRepositoryFactory,
    )
}
async fn drain(pool: &sqlx::PgPool, limit: u16) -> TestResult {
    for _ in 0..100 {
        handler(pool.clone()).execute(None, limit).await?;
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listing_geography_work WHERE pending)")
                .fetch_one(pool)
                .await?;
        if !pending {
            return Ok(());
        }
    }
    Err("fanout did not finish".into())
}
async fn fixture(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, Uuid, Uuid), Box<dyn std::error::Error>> {
    sqlx::query("INSERT INTO listing_geography_gate DEFAULT VALUES ON CONFLICT DO NOTHING")
        .execute(pool)
        .await?;
    let (id, _) = insert_product_with_created_event(pool).await?;
    let (source,party):(Uuid,Uuid)=sqlx::query_as("SELECT p.listing_source_id,s.operator_party_id FROM product_listings p JOIN listing_sources s USING(listing_source_id) WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(pool).await?;
    let location = Uuid::now_v7();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,position,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Secret premises',$3,$4,'ACTIVE','SERVICE:test','SERVICE:test')")
        .bind(location).bind(party).bind(json!({"reference_release":"iso-codes-4.20.1","address_text":"Private supplied address","country":"DE","subdivision":"DE-BY","postal_code":null}))
        .bind(json!({"point":{"latitude_degrees":48.1,"longitude_degrees":11.5},"precision":"PREMISES","accuracy_metres":null})).execute(pool).await?;
    sqlx::query("INSERT INTO listing_source_location_defaults(listing_source_id,party_id,party_location_id,business_evidence,revision,updated_actor) VALUES($1,$2,$3,$4,2,'SERVICE:test')")
        .bind(source).bind(party).bind(location).bind(json!({"scope":"ASSOCIATED_BUSINESS","reference":"protected evidence"})).execute(pool).await?;
    sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)")
        .bind(source.to_string())
        .execute(pool)
        .await?;
    Ok((id, source, location))
}
async fn acknowledge(pool: &sqlx::PgPool) -> TestResult {
    let rows: Vec<(Uuid, i64)> =
        sqlx::query_as("SELECT product_listing_id,projection_version FROM product_listings")
            .fetch_all(pool)
            .await?;
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    for (id, version) in rows {
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new()
            .in_transaction(&mut tx)
            .mark_projected(id.try_into()?, version)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
#[aura_integration_test(services=[SCHEMA])]
async fn malformed_applicability_and_geography_fail_closed_without_committing_projection() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        for corrupt_evidence in [true, false] {
            let (id, source, location) = fixture(&pool).await?;
            if corrupt_evidence {
                sqlx::query("UPDATE listing_source_location_defaults SET business_evidence=$2 WHERE listing_source_id=$1")
                    .bind(source).bind(json!({"scope":"ASSOCIATED_BUSINESS","reference":"  "}))
                    .execute(&pool).await?;
            } else {
                sqlx::query("UPDATE party_locations SET geography=$2 WHERE party_location_id=$1")
                    .bind(location).bind(json!({"reference_release":"iso-codes-4.20.1","address_text":null,"country":null,"subdivision":null,"postal_code":null}))
                    .execute(&pool).await?;
            }
        let work: Uuid = sqlx::query_scalar("SELECT work_id FROM listing_geography_work WHERE dependency_kind='SOURCE' AND dependency_id=$1")
            .bind(source.to_string()).fetch_one(&pool).await?;
        assert!(matches!(handler(pool.clone()).execute(Some(work.try_into()?), 100).await,
            Err(GeographyError::Storage(_))));
        let projected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM product_listing_geographies WHERE product_listing_id=$1)")
            .bind(id.into_uuid()).fetch_one(&pool).await?;
        assert!(!projected);
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        }
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "invalid persisted geography input: {result:?}"
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn shared_location_is_bounded_coalesced_and_does_not_rewrite_canonical_facts() {
    let result:TestResult=async{
        let pool=get_postgres_client().await;
        let (id,source,location)=fixture(&pool).await?;
        let before:(i64,time::OffsetDateTime)=sqlx::query_as("SELECT version,updated FROM product_listings WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;
        drain(&pool,1).await?;
        let (value,version):(serde_json::Value,i64)=sqlx::query_as("SELECT geography,geography_version FROM product_listing_geographies WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!("DE",value["business"]["country"]);assert!(value["inventory"].is_null());
        assert!(!value.to_string().contains("Private supplied"));assert!(!value.to_string().contains("evidence"));
        let after:(i64,time::OffsetDateTime)=sqlx::query_as("SELECT version,updated FROM product_listings WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;assert_eq!(before,after);
        // A first safe addition does not require suppressing other geographic queries.
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_some());
        acknowledge(&pool).await?;assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_some());
        let views=SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&[id]).await?;assert!(views[&id].business.is_some());
        // Repeated dependency signals update markers without downstream enrichment.
        sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)").bind(source.to_string()).execute(&pool).await?;drain(&pool,1).await?;
        let unchanged:i64=sqlx::query_scalar("SELECT geography_version FROM product_listing_geographies WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;assert_eq!(version,unchanged);
        // Retirement is visible before any fanout page/index acknowledgment.
        sqlx::query("UPDATE party_locations SET lifecycle='RETIRED',revision=revision+1 WHERE party_location_id=$1").bind(location).execute(&pool).await?;
        sqlx::query("INSERT INTO party_location_changes(party_location_id,party_id,revision,input_revision,lifecycle,actor) SELECT party_location_id,party_id,revision,input_revision,lifecycle,'SERVICE:test' FROM party_locations WHERE party_location_id=$1").bind(location).execute(&pool).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        assert!(SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&[id]).await?.is_empty());
        drain(&pool,1).await?;
        let retired:serde_json::Value=sqlx::query_scalar("SELECT geography FROM product_listing_geographies WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;
        assert!(retired["business"].is_null());assert!(retired["inventory"].is_null());
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());acknowledge(&pool).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_some());
        Ok(())
    }.await;
    assert!(result.is_ok(), "shared projection: {result:?}");
}
#[aura_integration_test(services=[SCHEMA])]
async fn ordinary_ingestion_preserves_query_permits_while_withdrawal_keeps_visibility_custody() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (id, source, _) = fixture(&pool).await?;
        drain(&pool, 100).await?;
        acknowledge(&pool).await?;
        let gate = SqlxGeographicQueryGate::new(pool.clone());
        let permit = gate.permit().await?.ok_or("missing initial permit")?;
        let old_version: i64 = sqlx::query_scalar("SELECT projection_version FROM product_listings WHERE product_listing_id=$1")
            .bind(id.into_uuid()).fetch_one(&pool).await?;
        for price in 1..=5_i64 {
            let event = EventId::new();
            let mut tx = pool.begin().await?;
            sqlx::query("UPDATE product_listings SET price_kind='MONETARY',price_amount=$2,price_currency='EUR',version=version+1,projection_version=projection_version+1,current_event_id=$3 WHERE product_listing_id=$1")
                .bind(id.into_uuid()).bind(price).bind(event.into_uuid()).execute(&mut *tx).await?;
            let previous = (price > 1).then(|| json!({"type":"MONETARY","amount":price-1,"currency":"EUR"}));
            sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'PRODUCT_LISTING_CHANGED','DOMAIN',1,$3,now())")
                .bind(event.into_uuid()).bind(id.into_uuid())
                .bind(json!({"pricing":{"price":{"previous":previous,"current":{"type":"MONETARY","amount":price,"currency":"EUR"}}}}))
                .execute(&mut *tx).await?;
            tx.commit().await?;
            assert_eq!(Some(permit), gate.permit().await?);
        }
        // Creation after the shared cursor has passed also keeps query permits;
        // its durable LISTING work later adds only currently permitted geography.
        let (created, _) = insert_product_with_created_event(&pool).await?;
        sqlx::query("UPDATE product_listings SET listing_source_id=$2 WHERE product_listing_id=$1")
            .bind(created.into_uuid()).bind(source).execute(&pool).await?;
        assert_eq!(Some(permit), gate.permit().await?);
        drain(&pool, 100).await?;
        assert_eq!(Some(permit), gate.permit().await?);
        assert!(SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&[created]).await?[&created].business.is_some());

        let event = EventId::new();
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE product_listings SET lifecycle='WITHDRAWN',availability=NULL,version=version+1,projection_version=projection_version+1,current_event_id=$2 WHERE product_listing_id=$1")
            .bind(id.into_uuid()).bind(event.into_uuid()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO product_listing_events(event_id,product_listing_id,event_type,event_group,event_type_schema_version,payload,event_time) VALUES($1,$2,'PRODUCT_LISTING_CHANGED','DOMAIN',1,$3,now())")
            .bind(event.into_uuid()).bind(id.into_uuid())
            .bind(json!({"availability":{"previous":"AVAILABLE","current":null},"lifecycle":{"transition":"WITHDRAWN","previousAvailability":"AVAILABLE"}}))
            .execute(&mut *tx).await?;
        tx.commit().await?;
        assert!(gate.permit().await?.is_none());
        // A later nonblocking wake-up cannot downgrade a pending withdrawal.
        sqlx::query("SELECT enqueue_listing_geography('LISTING',$1,false)")
            .bind(id.into_uuid().to_string()).execute(&pool).await?;
        assert!(gate.permit().await?.is_none());
        drain(&pool, 100).await?;
        assert!(gate.permit().await?.is_none());
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new().in_transaction(&mut tx).mark_projected(id, old_version).await?;
        tx.commit().await?;
        assert!(gate.permit().await?.is_none());
        acknowledge(&pool).await?;
        let restored_permit = gate.permit().await?.ok_or("missing post-withdrawal permit")?;
        assert!(restored_permit.epoch > permit.epoch);
        Ok(())
    }.await;
    assert!(
        result.is_ok(),
        "canonical catch-up and withdrawal custody: {result:?}"
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn crashed_leases_and_superseded_claims_recover_without_stale_writes() {
    let result:TestResult=async{
        let pool=get_postgres_client().await;let (id,source,_)=fixture(&pool).await?;
        let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_geography_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.to_string()).fetch_one(&pool).await?;
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let claim=SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).claim(Some(work.try_into()?)).await?.ok_or("missing claim")?;tx.commit().await?;
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;assert!(matches!(SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).claim(Some(work.try_into()?)).await,Err(GeographyError::Deferred)));tx.commit().await?;
        sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)").bind(source.to_string()).execute(&pool).await?;
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;let page=SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).page(&claim,1).await?;
        assert!(matches!(SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).checkpoint(&claim,&page).await,Err(GeographyError::Superseded)));drop(tx);
        // Simulate restart after a claimed page dies: expiry permits a new fenced claim.
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).claim(Some(work.try_into()?)).await?;tx.commit().await?;
        sqlx::query("UPDATE listing_geography_work SET lease_until=clock_timestamp()-interval '1 second' WHERE work_id=$1").bind(work).execute(&pool).await?;
        drain(&pool,1).await?;acknowledge(&pool).await?;
        assert!(SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&[id]).await?[&id].business.is_some());
        Ok(())
    }.await;
    assert!(result.is_ok(), "claim recovery: {result:?}");
}

#[aura_integration_test(services=[SCHEMA])]
async fn assignment_replacement_waits_for_projection_and_old_wakeups_read_current_authority() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let (id, source, old_location) = fixture(&pool).await?;
        let work: Uuid = sqlx::query_scalar("SELECT work_id FROM listing_geography_work WHERE dependency_kind='SOURCE' AND dependency_id=$1")
            .bind(source.to_string()).fetch_one(&pool).await?;
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let claim = SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).claim(Some(work.try_into()?)).await?.ok_or("missing claim")?;
        tx.commit().await?;
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let page = SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).page(&claim, 100).await?;
        // An independent management transaction cannot replace the assignment while
        // this page holds the source authority lock.
        let mut replacement = pool.begin().await?;
        sqlx::query("SET LOCAL lock_timeout='10ms'").execute(&mut *replacement).await?;
        let blocked = sqlx::query("SELECT listing_source_id FROM listing_sources WHERE listing_source_id=$1 FOR UPDATE")
            .bind(source).fetch_all(&mut *replacement).await.unwrap_err();
        assert_eq!(Some("55P03"), blocked.as_database_error().and_then(|e| e.code()).as_deref());
        replacement.rollback().await?;
        let writes = page.inputs.iter().map(|input| GeographyWrite { input: input.clone(), resolution: None, geography: derive(input, None) }).collect::<Vec<_>>();
        SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).apply(&writes).await?;
        SqlxGeographyProjectionStoreFactory.in_transaction(&mut tx).checkpoint(&claim, &page).await?;
        tx.commit().await?;
        let old_version: i64 = sqlx::query_scalar("SELECT geography_version FROM product_listing_geographies WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;
        let new_location = Uuid::now_v7();
        let mut replacement = pool.begin().await?;
        sqlx::query("SELECT listing_source_id FROM listing_sources WHERE listing_source_id=$1 FOR UPDATE").bind(source).fetch_all(&mut *replacement).await?;
        sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor) SELECT $1,party_id,'Replacement',$3,'ACTIVE','SERVICE:test','SERVICE:test' FROM party_locations WHERE party_location_id=$2")
            .bind(new_location).bind(old_location).bind(json!({"reference_release":"iso-codes-4.20.1","address_text":null,"country":"FR","subdivision":null,"postal_code":null})).execute(&mut *replacement).await?;
        sqlx::query("UPDATE listing_source_location_defaults SET party_location_id=$2,revision=revision+1 WHERE listing_source_id=$1").bind(source).bind(new_location).execute(&mut *replacement).await?;
        sqlx::query("INSERT INTO listing_source_location_default_changes(listing_source_id,revision,party_id,party_location_id,location_revision,business_association,advertised_inventory,actor) SELECT listing_source_id,revision,party_id,party_location_id,1,true,false,'SERVICE:test' FROM listing_source_location_defaults WHERE listing_source_id=$1").bind(source).execute(&mut *replacement).await?;
        replacement.commit().await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        // Even an old queued work identity recomputes the latest coalesced generation.
        handler(pool.clone()).execute(Some(work.try_into()?), 100).await?;
        drain(&pool, 100).await?;
        let current: serde_json::Value = sqlx::query_scalar("SELECT geography FROM product_listing_geographies WHERE product_listing_id=$1").bind(id.into_uuid()).fetch_one(&pool).await?;
        assert_eq!("FR", current["business"]["country"]);
        assert!(current["business"]["position"].is_null());
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new().in_transaction(&mut tx).mark_projected(id, old_version).await?;
        tx.commit().await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        acknowledge(&pool).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_some());
        Ok(())
    }.await;
    assert!(result.is_ok(), "assignment replacement fencing: {result:?}");
}
#[aura_integration_test(services=[SCHEMA])]
async fn pagination_covers_shared_sources_and_new_listings_and_rebuild_is_equivalent() {
    let result:TestResult=async{
        let pool=get_postgres_client().await;let delayed_id=ProductListingId::try_from(Uuid::parse_str("01900000-0000-7000-8000-000000000001")?)?;let (first,source,_)=fixture(&pool).await?;
        let work:Uuid=sqlx::query_scalar("SELECT work_id FROM listing_geography_work WHERE dependency_kind='SOURCE' AND dependency_id=$1").bind(source.to_string()).fetch_one(&pool).await?;
        let mut ids=vec![first];
        for _ in 0..4 {
            let (id,_)=insert_product_with_created_event(&pool).await?;
            sqlx::query("UPDATE product_listings SET listing_source_id=$2 WHERE product_listing_id=$1").bind(id.into_uuid()).bind(source).execute(&pool).await?;
            ids.push(id);
        }
        handler(pool.clone()).execute(Some(work.try_into()?),1).await?;
        let checkpoint:Option<Uuid>=sqlx::query_scalar("SELECT after_listing FROM listing_geography_work WHERE work_id=$1").bind(work).fetch_one(&pool).await?;
        assert!(checkpoint.is_some());assert!(delayed_id.into_uuid()<checkpoint.ok_or("missing checkpoint")?);
        // This creation arrives after fanout already checkpointed its first page.
        let (new_id,_)=insert_product_with_id(&pool,delayed_id).await?;
        sqlx::query("UPDATE product_listings SET listing_source_id=$2 WHERE product_listing_id=$1").bind(new_id.into_uuid()).bind(source).execute(&pool).await?;ids.push(new_id);
        drain(&pool,1).await?;acknowledge(&pool).await?;
        let before=SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?;
        assert_eq!(ids.len(),before.len());assert!(before.values().all(|g|g.business.is_some()&&g.inventory.is_none()));
        sqlx::query("SELECT enqueue_listing_geography('SOURCE',$1)").bind(source.to_string()).execute(&pool).await?;drain(&pool,2).await?;acknowledge(&pool).await?;
        assert_eq!(before,SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?);
        // Cursor and reverse lookup predicates have indexed access paths.
        let mut tx=pool.begin().await?;sqlx::query("SET LOCAL enable_seqscan=off").execute(&mut *tx).await?;
        let plans:Vec<(serde_json::Value,)> = sqlx::query_as("EXPLAIN (FORMAT JSON) SELECT product_listing_id FROM product_listings WHERE listing_source_id=$1 AND product_listing_id>$2 ORDER BY product_listing_id LIMIT 100")
            .bind(source).bind(first.into_uuid()).fetch_all(&mut *tx).await?;
        assert!(plans[0].0.to_string().contains("product_listings_source_geography_cursor_idx"));
        let plans:Vec<(serde_json::Value,)> = sqlx::query_as("EXPLAIN (FORMAT JSON) SELECT listing_source_id FROM listing_source_location_defaults WHERE party_location_id=$1")
            .bind(before[&first].business.as_ref().ok_or("missing business")?.location_id.into_uuid()).fetch_all(&mut *tx).await?;
        assert!(plans[0].0.to_string().contains("listing_source_location_defaults_location_idx"));
        Ok(())
    }.await;
    assert!(result.is_ok(), "paginated shared fanout: {result:?}");
}

async fn insert_product_with_created_event(
    pool: &sqlx::PgPool,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    insert_product_with_id(pool, ProductListingId::new()).await
}
async fn insert_product_with_id(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let event_id = EventId::new();
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Embedding party')",
    )
    .bind(party_id)
    .bind(format!("embedding-party-{party_id}"))
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Embedding source', $3)").bind(listing_source_id).bind(format!("embedding-source-{listing_source_id}")).bind(party_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')").bind(product_listing_id.into_uuid()).bind(format!(
                "embedding-product-{}",
                &product_listing_id.as_uuid().simple().to_string()[26..]
            )).bind(event_id.into_uuid()).bind(listing_source_id).bind(product_listing_id.to_string())
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id.into_uuid())
        .bind(product_listing_id.into_uuid())
        .bind(serde_json::json!({
            "listingSourceId": listing_source_core::ListingSourceId::try_from(listing_source_id)
                .expect("UUIDv7 fixture source").to_string(),
            "sourceListingId": product_listing_id.to_string(),
            "title": {"language": "de", "text": "Antiker Stuhl"},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": null
        }))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

#[aura_integration_test(services=[SCHEMA])]
async fn shared_resolution_and_expiry_never_require_per_listing_geocoding() {
    use geo_service::{
        geocoding::{
            CandidateSet, GeocodingCandidate, GeocodingOutcome, GeocodingProvenance,
            GeocodingRequest, GeocodingResult, ResultRetention,
        },
        resolution::*,
    };
    let result:TestResult=async{
        let pool=get_postgres_client().await;let (id,source,location)=fixture(&pool).await?;
        sqlx::query("UPDATE party_locations SET geography=NULL,position=NULL WHERE party_location_id=$1").bind(location).execute(&pool).await?;
        let owner:Uuid=sqlx::query_scalar("SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1").bind(source).fetch_one(&pool).await?;
        sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false").execute(&pool).await?;
        sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted) VALUES('TEST_G6',true,true,true)").execute(&pool).await?;
        let input=ResolutionInput::new(GeocodingRequest::new(geo::AddressText::new("Shared eligible address")?),ResolutionScope::new(ScopeKind::Party,Some(owner))?,RetentionPurpose::PartyLocation)?;
        let binding=ResolutionBinding {subject:ResolutionSubject::new(RetentionPurpose::PartyLocation,location)?,input_revision:1,guard:1};
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let resolution_id=SqlxResolutionRepositoryFactory.resolutions(&mut tx).bind(&input,binding).await?;
        let view=SqlxResolutionRepositoryFactory.resolution_reader(&mut tx).inspect(binding.subject).await?.ok_or("missing binding")?;tx.commit().await?;
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let ClaimOutcome::Claimed(claim)=SqlxResolutionRepositoryFactory.resolutions(&mut tx).claim(view.work,"TEST_G6",true).await? else {return Err("missing resolution claim".into());};tx.commit().await?;
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
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;
        assert!(SqlxResolutionRepositoryFactory.resolutions(&mut tx).complete(&claim,ResolutionCompletion::Evidence(evidence)).await?);tx.commit().await?;
        let mut ids=vec![id];for _ in 0..3 {let (new,_)=insert_product_with_created_event(&pool).await?;sqlx::query("UPDATE product_listings SET listing_source_id=$2 WHERE product_listing_id=$1").bind(new.into_uuid()).bind(source).execute(&pool).await?;ids.push(new);}
        drain(&pool,2).await?;acknowledge(&pool).await?;
        let views=SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?;
        assert_eq!(ids.len(),views.len());for g in views.values(){let business=g.business.as_ref().ok_or("missing association")?;assert_eq!(Some(geo::CountryCode::DEU),business.country);assert!(business.position.is_none());}
        let attempts:i32=sqlx::query_scalar("SELECT attempts FROM geo_resolutions WHERE resolution_id=$1").bind(resolution_id.into_uuid()).fetch_one(&pool).await?;assert_eq!(1,attempts);
        // Rights revocation atomically closes the gate even while old documents
        // and accepted evidence still exist. No provider call is needed to clear them.
        sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false WHERE profile='TEST_G6'").execute(&pool).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        drain(&pool,2).await?;acknowledge(&pool).await?;
        assert!(SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?.values().all(|g|g.business.is_none()));
        sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=true WHERE profile='TEST_G6'").execute(&pool).await?;
        drain(&pool,2).await?;acknowledge(&pool).await?;
        assert!(SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?.values().all(|g|g.business.as_ref().is_some_and(|g|g.country==Some(geo::CountryCode::DEU))));
        let before_expiry=SqlxGeographicQueryGate::new(pool.clone()).permit().await?.ok_or("missing pre-expiry permit")?;
        // Expiry closes public queries before maintenance emits the invalidation signal.
        sqlx::query("UPDATE geo_resolutions SET valid_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1").bind(resolution_id.into_uuid()).execute(&pool).await?;
        sqlx::query("UPDATE product_listing_geographies SET valid_until=clock_timestamp()-interval '1 second'").execute(&pool).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        // Ordinary catch-up may clear the expired derivative before maintenance.
        // A request spanning expiry must still reject its old permit after the
        // replacement index documents become visible and reopen the gate.
        for id in &ids {sqlx::query("SELECT enqueue_listing_geography('LISTING',$1,false)").bind(id.into_uuid().to_string()).execute(&pool).await?;}
        drain(&pool,2).await?;
        assert!(SqlxGeographicQueryGate::new(pool.clone()).permit().await?.is_none());
        acknowledge(&pool).await?;
        let after_expiry=SqlxGeographicQueryGate::new(pool.clone()).permit().await?.ok_or("missing post-expiry permit")?;
        assert!(after_expiry.epoch>before_expiry.epoch);
        let mut tx=SqlxUnitOfWork::new(pool.clone()).begin().await?;SqlxResolutionRepositoryFactory.resolutions(&mut tx).expire(100).await?;tx.commit().await?;
        drain(&pool,2).await?;acknowledge(&pool).await?;
        let views=SqlxEffectiveListingGeographyBatchReader::new(pool.clone()).read_many(&ids).await?;assert!(views.values().all(|g|g.business.is_none()));
        Ok(())
    }.await;
    assert!(result.is_ok(), "shared accepted resolution: {result:?}");
}
