use crawler::scraper::normalization::state::{ProductStateMappingRecord, StateMappingType};
use crawler::scraper::normalization::state_mapping_repository::{
    ProductStateMappingRepository, ProductStateMappingRepositoryImpl,
};
use product_listing_core::listing_availability::ListingAvailability;

use test_api::*;
use time::OffsetDateTime;

const POSTGRES: Postgres = Postgres::new_per_test("src/crawler/migrations");

fn make_record(
    raw: &str,
    normalized: Option<ListingAvailability>,
    mapping_type: StateMappingType,
) -> ProductStateMappingRecord {
    let now = OffsetDateTime::now_utc();
    ProductStateMappingRecord {
        raw: raw.to_string(),
        normalized,
        mapping_type,
        created: now,
        updated: now,
    }
}

// ---------------------------------------------------------------------------
// find_mapping
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_when_no_mapping_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let result = repository
        .find_mapping("nonexistent_raw_value")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_mapping_when_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record(
        "test_available",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );
    repository.insert_mapping(&record).await.unwrap();

    let result = repository
        .find_mapping("test_available")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.raw, "test_available");
    assert_eq!(result.normalized, Some(ListingAvailability::Available));
    assert_eq!(result.mapping_type, StateMappingType::Value);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_for_unknown_raw_when_other_mappings_exist_for_find() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record(
        "known_value",
        Some(ListingAvailability::SoldOut),
        StateMappingType::Value,
    );
    repository.insert_mapping(&record).await.unwrap();

    let result = repository.find_mapping("unknown_value").await.unwrap();

    assert!(result.is_none());
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_find_seed_data_value_mappings_for_find() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    // "available" is seeded in schema.sql
    let result = repository.find_mapping("available").await.unwrap().unwrap();

    assert_eq!(result.raw, "available");
    assert_eq!(result.normalized, Some(ListingAvailability::Available));
    assert_eq!(result.mapping_type, StateMappingType::Value);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_find_seed_data_regex_mappings_for_find() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    // A regex pattern seeded in schema.sql
    let result = repository
        .find_mapping(r"\b0\s+available\b")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.normalized, Some(ListingAvailability::SoldOut));
    assert_eq!(result.mapping_type, StateMappingType::Regex);
}

// ---------------------------------------------------------------------------
// insert_mapping
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_and_return_value_mapping_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record(
        "custom_sold",
        Some(ListingAvailability::SoldOut),
        StateMappingType::Value,
    );
    let returned = repository.insert_mapping(&record).await.unwrap();

    assert_eq!(returned.raw, "custom_sold");
    assert_eq!(returned.normalized, Some(ListingAvailability::SoldOut));
    assert_eq!(returned.mapping_type, StateMappingType::Value);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_and_return_regex_mapping_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record(
        r"\bcustom\s+pattern\b",
        Some(ListingAvailability::Reserved),
        StateMappingType::Regex,
    );
    let returned = repository.insert_mapping(&record).await.unwrap();

    assert_eq!(returned.raw, r"\bcustom\s+pattern\b");
    assert_eq!(returned.normalized, Some(ListingAvailability::Reserved));
    assert_eq!(returned.mapping_type, StateMappingType::Regex);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_preserve_created_and_updated_timestamps_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record("ts_test", None, StateMappingType::Value);
    let created_before = record.created;
    let updated_before = record.updated;

    let returned = repository.insert_mapping(&record).await.unwrap();

    // Timestamps must survive the round-trip (precision may be truncated to µs by Postgres)
    assert!(
        (returned.created - created_before).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        (returned.updated - updated_before).abs() < time::Duration::microseconds(1000),
        "updated timestamp drifted unexpectedly"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_allow_inserting_mappings_for_different_raw_values_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record_a = make_record(
        "raw_a",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );
    let record_b = make_record(
        "raw_b",
        Some(ListingAvailability::SoldOut),
        StateMappingType::Regex,
    );

    repository.insert_mapping(&record_a).await.unwrap();
    repository.insert_mapping(&record_b).await.unwrap();

    let result_a = repository.find_mapping("raw_a").await.unwrap().unwrap();
    let result_b = repository.find_mapping("raw_b").await.unwrap().unwrap();

    assert_eq!(result_a.normalized, Some(ListingAvailability::Available));
    assert_eq!(result_a.mapping_type, StateMappingType::Value);
    assert_eq!(result_b.normalized, Some(ListingAvailability::SoldOut));
    assert_eq!(result_b.mapping_type, StateMappingType::Regex);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_fail_when_inserting_duplicate_raw_value_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record = make_record(
        "dup_raw",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );

    repository.insert_mapping(&record).await.unwrap();

    let err = repository.insert_mapping(&record).await.unwrap_err();

    // Postgres unique-violation is SQLSTATE 23505, surfaced as a Database error by sqlx
    match err {
        sqlx::Error::Database(db_err) => {
            assert_eq!(
                db_err.code().as_deref(),
                Some("23505"),
                "Expected unique violation (23505), got: {:?}",
                db_err.code()
            );
        }
        other => panic!("Expected sqlx::Error::Database for PK violation, got: {other:?}"),
    }
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_all_product_state_variants_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let variants = [
        ("variant_listed", None),
        ("variant_available", Some(ListingAvailability::Available)),
        ("variant_reserved", Some(ListingAvailability::Reserved)),
        ("variant_sold", Some(ListingAvailability::SoldOut)),
        ("variant_removed", None),
        ("variant_unknown", None),
    ];

    for (raw, state) in &variants {
        let record = make_record(raw, *state, StateMappingType::Value);
        repository.insert_mapping(&record).await.unwrap();
    }

    for (raw, expected_state) in &variants {
        let found = repository.find_mapping(raw).await.unwrap().unwrap();
        assert_eq!(
            found.normalized, *expected_state,
            "State mismatch for raw value '{raw}'"
        );
    }
}

// ---------------------------------------------------------------------------
// update_mapping
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_replace_normalized_and_refresh_updated_timestamp_for_update() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let original = make_record(
        "upd_test",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );
    repository.insert_mapping(&original).await.unwrap();

    let inserted = repository.find_mapping("upd_test").await.unwrap().unwrap();

    // Small sleep so NOW() in the UPDATE is measurably later than the inserted `updated`
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let returned = repository
        .update_mapping(
            "upd_test",
            &Some(ListingAvailability::SoldOut),
            &StateMappingType::Value,
        )
        .await
        .unwrap();

    assert_eq!(returned.raw, "upd_test");
    assert_eq!(returned.normalized, Some(ListingAvailability::SoldOut));
    assert_ne!(returned.updated, inserted.updated);
    // created must remain unchanged
    assert!(
        (returned.created - original.created).abs() < time::Duration::microseconds(1000),
        "created timestamp must not change on update"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_update_mapping_type_from_value_to_regex_for_update() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let original = make_record(
        "type_change",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );
    repository.insert_mapping(&original).await.unwrap();

    let returned = repository
        .update_mapping(
            "type_change",
            &Some(ListingAvailability::Available),
            &StateMappingType::Regex,
        )
        .await
        .unwrap();

    assert_eq!(returned.mapping_type, StateMappingType::Regex);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_persist_updated_mapping_so_find_returns_new_value_for_update() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let original = make_record("upd_find", None, StateMappingType::Value);
    repository.insert_mapping(&original).await.unwrap();

    repository
        .update_mapping("upd_find", &None, &StateMappingType::Value)
        .await
        .unwrap();

    let found = repository.find_mapping("upd_find").await.unwrap().unwrap();

    assert_eq!(found.normalized, None);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_row_not_found_when_updating_non_existent_raw_value_for_update() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let err = repository
        .update_mapping(
            "does_not_exist",
            &Some(ListingAvailability::Available),
            &StateMappingType::Value,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(err, sqlx::Error::RowNotFound),
        "Expected RowNotFound when updating a raw value that does not exist, got: {err:?}"
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_only_update_targeted_raw_value_and_leave_others_intact_for_update() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    let record_a = make_record(
        "iso_a",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );
    let record_b = make_record(
        "iso_b",
        Some(ListingAvailability::Available),
        StateMappingType::Value,
    );

    repository.insert_mapping(&record_a).await.unwrap();
    repository.insert_mapping(&record_b).await.unwrap();

    repository
        .update_mapping(
            "iso_a",
            &Some(ListingAvailability::SoldOut),
            &StateMappingType::Regex,
        )
        .await
        .unwrap();

    let result_a = repository.find_mapping("iso_a").await.unwrap().unwrap();
    let result_b = repository.find_mapping("iso_b").await.unwrap().unwrap();

    assert_eq!(result_a.normalized, Some(ListingAvailability::SoldOut));
    assert_eq!(result_a.mapping_type, StateMappingType::Regex);
    assert_eq!(result_b.normalized, Some(ListingAvailability::Available));
    assert_eq!(result_b.mapping_type, StateMappingType::Value);
}

// ---------------------------------------------------------------------------
// round-trip (insert → find → update → find)
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
async fn should_preserve_all_fields_across_full_round_trip_for_repository() {
    let pool = get_postgres_client().await;
    let repository = ProductStateMappingRepositoryImpl::new(&pool);

    // 1. insert
    let record = make_record(
        "roundtrip",
        Some(ListingAvailability::Reserved),
        StateMappingType::Value,
    );
    let inserted = repository.insert_mapping(&record).await.unwrap();

    assert_eq!(inserted.raw, "roundtrip");
    assert_eq!(inserted.normalized, Some(ListingAvailability::Reserved));
    assert_eq!(inserted.mapping_type, StateMappingType::Value);

    // 2. find after insert
    let found_after_insert = repository.find_mapping("roundtrip").await.unwrap().unwrap();

    assert_eq!(found_after_insert.raw, "roundtrip");
    assert_eq!(
        found_after_insert.normalized,
        Some(ListingAvailability::Reserved)
    );
    assert_eq!(found_after_insert.mapping_type, StateMappingType::Value);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 3. update
    let updated = repository
        .update_mapping(
            "roundtrip",
            &Some(ListingAvailability::SoldOut),
            &StateMappingType::Regex,
        )
        .await
        .unwrap();

    assert_eq!(updated.normalized, Some(ListingAvailability::SoldOut));
    assert_eq!(updated.mapping_type, StateMappingType::Regex);
    assert_ne!(updated.updated, inserted.updated);

    // 4. find after update
    let found_after_update = repository.find_mapping("roundtrip").await.unwrap().unwrap();

    assert_eq!(
        found_after_update.normalized,
        Some(ListingAvailability::SoldOut)
    );
    assert_eq!(found_after_update.mapping_type, StateMappingType::Regex);
    assert_eq!(found_after_update.created, found_after_insert.created);
}
