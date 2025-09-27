use common::{query::range_query::RangeQuery, user_id::UserId};
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
        .query_watchlist_records(&expected.user_id, &Default::default(), true)
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

    let expected = records.into_iter().skip(37).collect::<Vec<_>>();
    let query = RangeQuery {
        min: Some(expected[0].created),
        max: None,
    };

    let actual = repository
        .query_watchlist_records(&user_id, &query, true)
        .await
        .unwrap();
    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_higher_bounded_created_for_scan_index_true() {
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

    let expected = records.into_iter().take(37).collect::<Vec<_>>();
    let query = RangeQuery {
        min: None,
        max: Some(expected[36].created),
    };

    let actual = repository
        .query_watchlist_records(&user_id, &query, true)
        .await
        .unwrap();
    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_lower_higher_bounded_created_for_scan_index_true() {
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

    let expected = records.into_iter().skip(2).take(37).collect::<Vec<_>>();
    let query = RangeQuery {
        min: Some(expected[0].created),
        max: Some(expected[36].created),
    };

    let actual = repository
        .query_watchlist_records(&user_id, &query, true)
        .await
        .unwrap();
    assert_eq!(expected, actual);
}
