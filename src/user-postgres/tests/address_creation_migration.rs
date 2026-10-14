use application::transaction::UnitOfWork;
use geo::{AddressText, GeographicDescription};
use platform_postgres::SqlxUnitOfWork;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::{user_address::*, user_id::UserId};
use user_postgres::SqlxAddressBookFactory;
use user_service::address_book::{AddressBookFactory, UserAddressReader, UserAddressRepository};

const SCHEMA: Postgres = Postgres::new("migrations");
const PRIOR: &str = include_str!("../../../migrations/20261010120000_geography.sql");
const CLEANUP: &str =
    include_str!("../../../migrations/20261014120000_remove_geography_creation_receipts.sql");

#[aura_integration_test(services = [SCHEMA])]
async fn keyless_writes_support_both_schemas_and_cleanup_preserves_addresses_and_tombstones() {
    let pool = get_postgres_client().await;
    let mut tx = SqlxUnitOfWork::new(pool).begin().await.unwrap();
    // Reconstruct the previous schema only inside this rolled-back transaction,
    // reusing the applied migration's exact constraint and trigger definitions.
    let constraint = PRIOR
        .split("    CONSTRAINT user_addresses_creation_receipt_check ")
        .nth(1)
        .unwrap()
        .split("    label text,")
        .next()
        .unwrap()
        .trim()
        .trim_end_matches(',');
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER TABLE user_addresses ADD COLUMN create_key_hash text, ADD COLUMN create_fingerprint text, ADD CONSTRAINT user_addresses_creation_receipt_check {constraint};")))
        .execute(tx.connection()).await.unwrap();
    let guards = PRIOR
        .split("CREATE FUNCTION guard_user_address_identity()")
        .nth(1)
        .unwrap()
        .split("-- Bounded derived read state.")
        .next()
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP TRIGGER user_address_identity ON user_addresses; DROP FUNCTION guard_user_address_identity(); CREATE FUNCTION guard_user_address_identity(){guards}")))
        .execute(tx.connection()).await.unwrap();
    let receipts = PRIOR
        .split("CREATE TABLE party_location_create_receipts")
        .nth(1)
        .unwrap()
        .split("-- Minimal immutable change signal")
        .next()
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE TABLE party_location_create_receipts{receipts}"
    )))
    .execute(tx.connection())
    .await
    .unwrap();

    let owner = UserId::new();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(owner.into_uuid())
        .bind(format!("{owner}@example.test"))
        .execute(tx.connection())
        .await
        .unwrap();
    let party = uuid::Uuid::now_v7();
    let location = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,'receipt-cleanup','Cleanup test')")
        .bind(party).execute(tx.connection()).await.unwrap();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Site','ACTIVE','test','test')")
        .bind(location).bind(party).execute(tx.connection()).await.unwrap();
    sqlx::query("INSERT INTO party_location_changes(party_location_id,revision,party_id,input_revision,lifecycle,actor) VALUES($1,1,$2,1,'ACTIVE','test')")
        .bind(location).bind(party).execute(tx.connection()).await.unwrap();
    sqlx::query("INSERT INTO party_location_create_receipts(party_id,actor,idempotency_key,command,result) VALUES($1,'test','legacy','{}','{}')")
        .bind(party).execute(tx.connection()).await.unwrap();
    let original = UserAddressId::new();
    let deleted = UserAddressId::new();
    sqlx::query("INSERT INTO user_addresses(address_id,user_id,label,geography,create_key_hash,create_fingerprint) VALUES($1,$2,'Home',$3,repeat('a',64),repeat('b',64))")
        .bind(original.into_uuid()).bind(owner.into_uuid())
        .bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":"Synthetic home"}))
        .execute(tx.connection()).await.unwrap();
    sqlx::query("INSERT INTO user_addresses(address_id,user_id,label,geography,create_key_hash,create_fingerprint) VALUES($1,$2,'Temporary',$3,repeat('c',64),repeat('d',64))")
        .bind(deleted.into_uuid()).bind(owner.into_uuid())
        .bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":"Synthetic temporary address"}))
        .execute(tx.connection()).await.unwrap();
    SqlxAddressBookFactory
        .repository(&mut tx)
        .set_destination(owner, &BrowsingDestination::Address(original), 1)
        .await
        .unwrap();

    // The keyless adapter must still edit and erase entries from the old runtime.
    let mut legacy = SqlxAddressBookFactory
        .repository(&mut tx)
        .find(owner, original)
        .await
        .unwrap()
        .unwrap();
    legacy
        .address
        .replace(
            UserAddressLabel::new("Renamed".into()).unwrap(),
            legacy.address.geography().clone(),
        )
        .unwrap();
    let updated = SqlxAddressBookFactory
        .repository(&mut tx)
        .update(&legacy.address, legacy.version)
        .await
        .unwrap();
    assert_eq!(2, updated.version);
    SqlxAddressBookFactory
        .repository(&mut tx)
        .delete(owner, deleted, 1)
        .await
        .unwrap();
    let tombstone: (
        bool,
        Option<String>,
        Option<serde_json::Value>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT deleted,label,geography,create_fingerprint FROM user_addresses WHERE address_id=$1",
    )
    .bind(deleted.into_uuid())
    .fetch_one(tx.connection())
    .await
    .unwrap();
    assert_eq!((true, None, None, None), tombstone);

    // The replacement adapter writes ordinary equal-content resources while the
    // old optional receipt columns and immutable-key guards are still present.
    let mut ids = Vec::new();
    for _ in 0..2 {
        let address = UserAddress::create(
            UserAddressId::new(),
            owner,
            UserAddressLabel::new("Home".into()).unwrap(),
            GeographicDescription::new(
                Some(AddressText::new("Synthetic home").unwrap()),
                None,
                None,
                None,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        ids.push(
            SqlxAddressBookFactory
                .repository(&mut tx)
                .insert(&address)
                .await
                .unwrap()
                .address
                .id(),
        );
    }
    assert_ne!(ids[0], ids[1]);
    let before = SqlxAddressBookFactory
        .reader(&mut tx)
        .get(owner, original)
        .await
        .unwrap()
        .unwrap();
    sqlx::raw_sql(CLEANUP)
        .execute(tx.connection())
        .await
        .unwrap();
    let after = SqlxAddressBookFactory
        .reader(&mut tx)
        .get(owner, original)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.id, after.id);
    assert_eq!(before.label, after.label);
    assert_eq!(before.geography, after.geography);
    assert_eq!(before.version, after.version);
    assert!(after.preferred);
    assert!(
        SqlxAddressBookFactory
            .reader(&mut tx)
            .get(owner, deleted)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        4,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM user_addresses")
            .fetch_one(tx.connection())
            .await
            .unwrap()
    );
    assert_eq!(
        3,
        SqlxAddressBookFactory
            .reader(&mut tx)
            .list(owner, None, 100)
            .await
            .unwrap()
            .items
            .len()
    );
    assert_eq!(0, sqlx::query_scalar::<_, i64>("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='user_addresses' AND column_name IN ('create_key_hash','create_fingerprint')").fetch_one(tx.connection()).await.unwrap());
    assert!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT to_regclass('party_location_create_receipts')::text"
        )
        .fetch_one(tx.connection())
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT revision FROM party_locations WHERE party_location_id=$1"
        )
        .bind(location)
        .fetch_one(tx.connection())
        .await
        .unwrap()
    );
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM party_location_changes WHERE party_location_id=$1"
        )
        .bind(location)
        .fetch_one(tx.connection())
        .await
        .unwrap()
    );
    let changed =
        UserAddress::create(UserAddressId::new(), owner, after.label, after.geography).unwrap();
    SqlxAddressBookFactory
        .repository(&mut tx)
        .insert(&changed)
        .await
        .unwrap();
    assert_eq!(
        4,
        SqlxAddressBookFactory
            .reader(&mut tx)
            .list(owner, None, 100)
            .await
            .unwrap()
            .items
            .len()
    );
    // Dropping the uncommitted transaction restores the current schema.
}
