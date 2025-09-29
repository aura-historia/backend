use common::user_id::UserId;
use fake::{Fake, Faker};
use item_watchlist::{
    record::{WatchlistItemRecord, mk_pk},
    repository::{WatchlistItemDynamoDbRepository, WatchlistItemDynamoDbRepositoryImpl},
};
use test_api::*;

async fn get_repository() -> WatchlistItemDynamoDbRepositoryImpl<'static> {
    WatchlistItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_watchlist_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<WatchlistItemRecord>();
    let _ = repository
        .put_watchlist_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(&expected.user_id, &expected.created)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_delete_watchlist_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<WatchlistItemRecord>();
    let _ = repository
        .put_watchlist_record(expected.clone())
        .await
        .unwrap();

    let _ = repository
        .delete_watchlist_record(&expected.user_id, &expected.created)
        .await
        .unwrap();

    let actual = repository
        .query_watchlist_records(&expected.user_id, &Default::default(), 42, true)
        .await
        .unwrap();
    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_lower_bounded_created_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let expected = records.clone().into_iter().skip(37).collect::<Vec<_>>();

    let actual = repository
        .query_watchlist_records(&user_id, &Some(records.get(36).unwrap().created), 100, true)
        .await
        .unwrap();
    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_not_bound_created_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records(&user_id, &None, 100, true)
        .await
        .unwrap();
    assert_eq!(records, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_and_respect_limit() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records(&user_id, &None, 10, true)
        .await
        .unwrap();

    assert_eq!(10, actual.len());
    assert_eq!(records.into_iter().take(10).collect::<Vec<_>>(), actual);
}
