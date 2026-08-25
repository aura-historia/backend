use crawler::scraper::normalization::listing_availability_mapping::{
    ListingAvailabilityDecisionKind, ListingAvailabilityMappingRecord,
    ListingAvailabilityMappingType,
};
use crawler::scraper::normalization::listing_availability_mapping_repository::{
    ListingAvailabilityMappingRepository, ListingAvailabilityMappingRepositoryImpl,
};
use product_listing_core::listing_availability::ListingAvailability;
use test_api::*;
use time::OffsetDateTime;

const POSTGRES: Postgres = Postgres::new_per_test("src/crawler/migrations");

fn make_record(
    raw: &str,
    availability: Option<ListingAvailability>,
    mapping_type: ListingAvailabilityMappingType,
) -> ListingAvailabilityMappingRecord {
    let now = OffsetDateTime::now_utc();
    ListingAvailabilityMappingRecord {
        raw: raw.to_owned(),
        availability,
        mapping_type,
        decision_kind: match availability {
            Some(_) => ListingAvailabilityDecisionKind::Availability,
            None => ListingAvailabilityDecisionKind::NoAssertion,
        },
        created: now,
        updated: now,
    }
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_none_when_no_mapping_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);

    assert!(
        repository
            .find_mapping("nonexistent_raw_value")
            .await
            .unwrap()
            .is_none()
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_round_trip_availability_mapping_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);
    let record = make_record(
        "custom_available",
        Some(ListingAvailability::Available),
        ListingAvailabilityMappingType::Value,
    );

    let inserted = repository.insert_mapping(&record).await.unwrap();
    let found = repository
        .find_mapping("custom_available")
        .await
        .unwrap()
        .unwrap();

    for returned in [inserted, found] {
        assert_eq!(returned.raw, "custom_available");
        assert_eq!(returned.availability, Some(ListingAvailability::Available));
        assert_eq!(
            returned.decision_kind,
            ListingAvailabilityDecisionKind::Availability
        );
        assert_eq!(returned.mapping_type, ListingAvailabilityMappingType::Value);
    }
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_round_trip_no_assertion_mapping_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);
    let record = make_record("no_assertion", None, ListingAvailabilityMappingType::Value);

    let inserted = repository.insert_mapping(&record).await.unwrap();

    assert_eq!(inserted.raw, "no_assertion");
    assert_eq!(inserted.availability, None);
    assert_eq!(
        inserted.decision_kind,
        ListingAvailabilityDecisionKind::NoAssertion
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_seeded_out_of_stock_regex_mapping_for_find() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);

    let result = repository
        .find_mapping(r"\b0\s+available\b")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.availability, Some(ListingAvailability::OutOfStock));
    assert_eq!(result.mapping_type, ListingAvailabilityMappingType::Regex);
    assert_eq!(
        result.decision_kind,
        ListingAvailabilityDecisionKind::Availability
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_update_availability_and_preserve_created_timestamp() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);
    let original = make_record(
        "update_availability",
        Some(ListingAvailability::Available),
        ListingAvailabilityMappingType::Value,
    );
    let inserted = repository.insert_mapping(&original).await.unwrap();
    let replacement = ListingAvailabilityMappingRecord {
        availability: Some(ListingAvailability::SoldOut),
        mapping_type: ListingAvailabilityMappingType::Regex,
        decision_kind: ListingAvailabilityDecisionKind::Availability,
        ..inserted.clone()
    };

    let updated = repository.update_mapping(&replacement).await.unwrap();

    assert_eq!(updated.availability, Some(ListingAvailability::SoldOut));
    assert_eq!(updated.mapping_type, ListingAvailabilityMappingType::Regex);
    assert_eq!(
        updated.decision_kind,
        ListingAvailabilityDecisionKind::Availability
    );
    assert_eq!(updated.created, inserted.created);
    assert!(updated.updated >= inserted.updated);
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_update_mapping_to_no_assertion() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);
    let inserted = repository
        .insert_mapping(&make_record(
            "clear_assertion",
            Some(ListingAvailability::Available),
            ListingAvailabilityMappingType::Value,
        ))
        .await
        .unwrap();
    let no_assertion = ListingAvailabilityMappingRecord {
        availability: None,
        decision_kind: ListingAvailabilityDecisionKind::NoAssertion,
        ..inserted
    };

    let updated = repository.update_mapping(&no_assertion).await.unwrap();

    assert_eq!(updated.availability, None);
    assert_eq!(
        updated.decision_kind,
        ListingAvailabilityDecisionKind::NoAssertion
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_return_row_not_found_when_updating_unknown_mapping() {
    let pool = get_postgres_client().await;
    let repository = ListingAvailabilityMappingRepositoryImpl::new(&pool);

    let error = repository
        .update_mapping(&make_record(
            "does_not_exist",
            Some(ListingAvailability::Available),
            ListingAvailabilityMappingType::Value,
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, sqlx::Error::RowNotFound));
}
