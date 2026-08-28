use crawler::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, ListingSourceUrlPatternRepositoryImpl,
};
use listing_source_core::Domain;
use listing_source_core::ListingSourceId;

use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

// ---------------------------------------------------------------------------
// find_pattern
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_return_none_when_no_pattern_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool);
    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();

    let result = repository.find_pattern(&listing_source_id).await.unwrap();

    assert!(result.is_none());
}

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_return_pattern_when_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool.clone());
    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_domain = Domain::try_from("example.com").unwrap();
    let pattern = r"/product/\d+";

    repository
        .save_pattern(&listing_source_id, &listing_source_domain, Some(pattern))
        .await
        .unwrap();

    let result = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.listing_source_id, listing_source_id);
    assert_eq!(result.listing_source_domain, listing_source_domain);
    assert_eq!(result.url_pattern.unwrap(), pattern);
}

// ---------------------------------------------------------------------------
// save_pattern
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_persist_and_return_pattern_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool);

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_domain = Domain::try_from("insert-example.com").unwrap();
    let pattern = r"/item/\w+";

    repository
        .save_pattern(&listing_source_id, &listing_source_domain, Some(pattern))
        .await
        .unwrap();

    let returned = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(returned.listing_source_id, listing_source_id);
    assert_eq!(returned.listing_source_domain, listing_source_domain);
    assert_eq!(returned.url_pattern.unwrap(), pattern);
}

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_preserve_created_and_updated_timestamps_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool);

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_domain = Domain::try_from("ts-example.com").unwrap();
    let pattern = "/ts-item";

    repository
        .save_pattern(&listing_source_id, &listing_source_domain, Some(pattern))
        .await
        .unwrap();
    let record1 = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    repository
        .save_pattern(
            &listing_source_id,
            &listing_source_domain,
            Some("/ts-item-new"),
        )
        .await
        .unwrap();
    let record2 = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert!(
        (record2.created - record1.created).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        record2.updated > record1.updated,
        "updated timestamp should be strictly newer after an update"
    );
}

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_allow_clearing_pattern() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool);

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_domain = Domain::try_from("clear-example.com").unwrap();

    repository
        .save_pattern(
            &listing_source_id,
            &listing_source_domain,
            Some("/clear-item"),
        )
        .await
        .unwrap();

    // Explicitly clear pattern
    repository
        .save_pattern(&listing_source_id, &listing_source_domain, None)
        .await
        .unwrap();

    let returned = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(returned.listing_source_id, listing_source_id);
    assert_eq!(returned.listing_source_domain, listing_source_domain);
    assert!(returned.url_pattern.is_none());
}

// ---------------------------------------------------------------------------
// mark_as_crawled
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [POSTGRES])]
#[serial_test::serial]
async fn should_mark_pattern_as_crawled() {
    let pool = get_postgres_client().await;
    let repository = ListingSourceUrlPatternRepositoryImpl::new(pool);

    let listing_source_id: ListingSourceId = uuid::Uuid::new_v4().into();
    let listing_source_domain = Domain::try_from("mark-example.com").unwrap();

    // Mark as crawled directly without a pattern
    repository
        .mark_as_crawled(&listing_source_id, &listing_source_domain)
        .await
        .unwrap();

    let record = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(record.last_crawled.is_some());
    assert!(record.url_pattern.is_none());

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Mark again
    repository
        .mark_as_crawled(&listing_source_id, &listing_source_domain)
        .await
        .unwrap();

    let record2 = repository
        .find_pattern(&listing_source_id)
        .await
        .unwrap()
        .unwrap();
    assert!(record2.last_crawled.unwrap() > record.last_crawled.unwrap());
}
